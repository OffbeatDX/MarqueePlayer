//! Tag editing in View metadata (metadata/view.rs; the writing itself is `library::edit`).
//!
//! Off unless Settings > Library > Edit tags is on: with it off, nothing here runs and the
//! panel is the read-only viewer. With it on, a field's values (or a track row of an
//! album's table, or the album's fields for every track) are edited in place; nothing is
//! written until Save. Edits are a draft (`Draft`): per file, per field, the values it
//! had and the values it will have, and the front cover to put in or take out. "Review
//! and save" lists every change, file by file; Save writes them one file after another on
//! a thread of its own (`save`), each through the editor's checked, atomic replacement,
//! with progress and Cancel between files; then the result says what was written, and
//! what was not and why. Every save can be undone from the panel's Edit history while the
//! journal keeps it (`library::edit::journal`).
//!
//! The library takes the new tags through the same path as any other retag: the folders
//! that changed are scanned (`scanning::rescan_changed`, as the folder watcher does), so
//! ids, plays, favourites, ratings and playlists stay, and an album whose identity changed
//! is regrouped by the scanner's rules.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use library::edit::{self, Expect, FileEdit, Journal, PictureEdit};
use library::meta::{self, Field};
use slint::ComponentHandle;

use crate::state::{self, State};

/// The journal and the records of writes under way, in the data folder.
pub fn journal(st: &State) -> Journal {
    Journal::new(&st.paths.edits())
}

/// At start: finish or undo any write a crash interrupted. Looks only at the records in
/// the data folder (none, normally: then it reads one empty folder and is done).
pub fn recover_at_start(edits: &Path) {
    let j = Journal::new(edits);
    if !j.inflight().is_dir() {
        return;
    }
    for line in edit::atomic::recover(&j.inflight()) {
        log::warn!("tag editor, after an interrupted save: {line}");
    }
}

pub fn enabled(st: &State) -> bool {
    st.settings.borrow().tag_editing
}

/// Settings > Library: the switch, and what the undo history holds.
pub fn sync_settings(st: &State) {
    let Some(app) = st.app.upgrade() else { return };
    let on = enabled(st);
    let l = app.global::<crate::Library>();
    l.set_tag_editing(on);
    let text = if on || journal(st).dir().join("saves").is_dir() {
        let (bytes, saves) = journal(st).size();
        match saves {
            0 => "Nothing kept yet. The last 100 saves are kept, at most 256 MB with the covers they replaced.".to_owned(),
            n => format!(
                "{} kept, {}. The last {} saves are kept, at most {} MB with the covers they replaced.",
                if n == 1 { "1 save".to_owned() } else { format!("{n} saves") },
                library::edit::cover::size_text(bytes),
                library::edit::journal::MAX_SAVES,
                library::edit::journal::MAX_BYTES >> 20
            ),
        }
    } else {
        String::new()
    };
    l.set_edit_history(text.into());
}

pub fn toggle(st: &Rc<State>) {
    {
        let mut s = st.settings.borrow_mut();
        s.tag_editing = !s.tag_editing;
    }
    st.save_settings_soon();
    log::info!("tag editing {}", if enabled(st) { "on" } else { "off" });
    sync_settings(st);
}

/// Forget every save (they can no longer be undone).
pub fn clear_history(st: &Rc<State>) {
    if saving() {
        return;
    }
    journal(st).clear();
    sync_settings(st);
    crate::views::flash(st, "Undo history cleared");
}

// ---- The draft ------------------------------------------------------------------------

/// A field's values before and after.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub old: Vec<String>,
    pub new: Vec<String>,
}

/// The front cover: a new one (made ready, `edit::cover`), or none.
#[derive(Clone, Debug, PartialEq)]
pub enum Cover {
    Set(Rc<edit::cover::Prepared>),
    Remove,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FileDraft {
    /// The file's name, for the review ("03 Patch Three.flac").
    pub name: String,
    pub fields: BTreeMap<String, Change>,
    pub cover: Option<Cover>,
    /// What the file must still be when saved (as read for the draft).
    pub expect: Option<Expect>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Draft {
    pub files: BTreeMap<String, FileDraft>,
}

/// A field as a person reads it ("Album artist").
pub fn label(key: &str) -> String {
    meta::by_key(key).map_or_else(|| key.to_owned(), |d| d.label.to_owned())
}

fn shown(v: &[String]) -> String {
    if v.is_empty() { "nothing".into() } else { v.iter().map(|s| format!("\u{201c}{s}\u{201d}")).collect::<Vec<_>>().join(", ") }
}

impl Draft {
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Set a field of a file: a change back to what it was is no change.
    pub fn set(&mut self, path: &str, name: &str, expect: Option<Expect>, key: &str, old: Vec<String>, new: Vec<String>) {
        let f = self.files.entry(path.to_owned()).or_insert_with(|| FileDraft { name: name.to_owned(), expect: expect.clone(), ..Default::default() });
        if f.expect.is_none() {
            f.expect = expect;
        }
        let old = f.fields.get(key).map(|c| c.old.clone()).unwrap_or(old);
        if new == old {
            f.fields.remove(key);
        } else {
            f.fields.insert(key.to_owned(), Change { old, new });
        }
        self.tidy(path);
    }

    pub fn set_cover(&mut self, path: &str, name: &str, expect: Option<Expect>, cover: Option<Cover>) {
        let f = self.files.entry(path.to_owned()).or_insert_with(|| FileDraft { name: name.to_owned(), expect: expect.clone(), ..Default::default() });
        f.cover = cover;
        self.tidy(path);
    }

    fn tidy(&mut self, path: &str) {
        if self.files.get(path).is_some_and(|f| f.fields.is_empty() && f.cover.is_none()) {
            self.files.remove(path);
        }
    }

    pub fn field(&self, path: &str, key: &str) -> Option<&Change> {
        self.files.get(path).and_then(|f| f.fields.get(key))
    }

    pub fn file(&self, path: &str) -> Option<&FileDraft> {
        self.files.get(path)
    }

    /// (changes, files).
    pub fn count(&self) -> (usize, usize) {
        (self.files.values().map(|f| f.fields.len() + usize::from(f.cover.is_some())).sum(), self.files.len())
    }

    /// "3 unsaved changes", "12 unsaved changes in 4 files".
    pub fn status(&self) -> String {
        let (n, files) = self.count();
        let changes = if n == 1 { "1 unsaved change".to_owned() } else { format!("{n} unsaved changes") };
        if files > 1 { format!("{changes} in {files} files") } else { changes }
    }

    /// What changes in a file, one line each: "Album artist: “A” → “B”".
    pub fn lines(f: &FileDraft) -> Vec<String> {
        let mut out: Vec<String> = f
            .fields
            .iter()
            .map(|(k, c)| match (c.old.is_empty(), c.new.is_empty()) {
                (_, true) => format!("{}: {} removed", label(k), shown(&c.old)),
                (true, false) => format!("{}: {} (new)", label(k), shown(&c.new)),
                _ => format!("{}: {} \u{2192} {}", label(k), shown(&c.old), shown(&c.new)),
            })
            .collect();
        match &f.cover {
            Some(Cover::Set(p)) => out.push(format!("Front cover: {}", p.describe().0)),
            Some(Cover::Remove) => out.push("Front cover: removed".into()),
            None => {}
        }
        out
    }

    /// The edits to write.
    pub fn edits(&self) -> Vec<FileEdit> {
        self.files
            .iter()
            .map(|(path, f)| FileEdit {
                path: PathBuf::from(path),
                fields: f.fields.iter().map(|(k, c)| (k.clone(), c.new.clone())).collect(),
                raw: Vec::new(),
                pictures: f.cover.as_ref().map(|c| match c {
                    Cover::Set(p) => PictureEdit { remove_front: true, add: vec![p.picture.clone()] },
                    Cover::Remove => PictureEdit { remove_front: true, add: Vec::new() },
                }),
                expect: f.expect.clone().unwrap_or(Expect::Any),
                summary: Draft::lines(f),
            })
            .collect()
    }

    /// What the save is called in the journal and Edit history.
    pub fn label(&self, what: &str) -> String {
        let (_, files) = self.count();
        let keys: Vec<String> = {
            let mut k: Vec<String> = self.files.values().flat_map(|f| f.fields.keys().map(|k| label(k).to_lowercase())).collect();
            if self.files.values().any(|f| f.cover.is_some()) {
                k.push("front cover".into());
            }
            k.sort();
            k.dedup();
            k
        };
        let first = keys.first().map(|k| k[..1].to_uppercase() + &k[1..]).unwrap_or_default();
        let rest = if keys.len() > 1 { format!(" and {} more", keys.len() - 1) } else { String::new() };
        if files > 1 { format!("{what}: {first}{rest} on {files} files") } else { format!("{what}: {first}{rest}") }
    }
}

/// A field's values as an editor starts from them: the first block's (the library's),
/// and for numbers, the number or the total out of "3/12".
pub fn values_of(fields: &[Field], key: &str) -> Vec<String> {
    let get = |k: &str| fields.iter().find(|f| f.def.is_some_and(|d| d.key == k)).map(|f| f.values().iter().map(|v| v.trim().to_owned()).filter(|v| !v.is_empty()).collect::<Vec<_>>()).unwrap_or_default();
    match key {
        "TRACKNUMBER" | "DISCNUMBER" => get(key).into_iter().map(|v| v.split('/').next().unwrap_or("").trim().to_owned()).filter(|v| !v.is_empty()).take(1).collect(),
        "TRACKTOTAL" | "DISCTOTAL" => {
            let own = get(key);
            if !own.is_empty() {
                return own;
            }
            let number = if key == "TRACKTOTAL" { "TRACKNUMBER" } else { "DISCNUMBER" };
            get(number).first().and_then(|v| v.split_once('/')).map(|(_, t)| t.trim().to_owned()).filter(|t| !t.is_empty()).into_iter().collect()
        }
        "COMPILATION" => get(key).into_iter().take(1).map(|v| if matches!(v.to_ascii_lowercase().as_str(), "1" | "yes" | "true") { "1".into() } else { "0".into() }).collect(),
        _ => get(key),
    }
}

/// A value as typed: checked, and "3/12" typed as a track number taken as number and total.
pub fn read_values(key: &str, typed: &[String]) -> Result<Vec<(String, Vec<String>)>, String> {
    let mut extra = Vec::new();
    let mut vals: Vec<String> = typed.to_vec();
    if matches!(key, "TRACKNUMBER" | "DISCNUMBER")
        && let [one] = vals.as_slice()
        && let Some((n, t)) = one.split_once('/')
        && !t.trim().is_empty()
    {
        let total = if key == "TRACKNUMBER" { "TRACKTOTAL" } else { "DISCTOTAL" };
        extra.push((total.to_owned(), edit::check(total, &[t.trim().to_owned()])?));
        vals = vec![n.trim().to_owned()];
    }
    let mut out = vec![(key.to_owned(), edit::check(key, &vals)?)];
    out.extend(extra);
    Ok(out)
}

// ---- Picking a cover --------------------------------------------------------------------

thread_local! {
    /// The bench's answer to the file picker (bench builds never open a dialog).
    pub static PICKED: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

/// Ask for an image (a file dialog; the bench's path in bench runs).
pub fn pick_image() -> Option<PathBuf> {
    if let Some(p) = PICKED.with(|p| p.borrow_mut().take()) {
        return Some(p);
    }
    if !crate::dialog_allowed("pick a cover image") {
        return None;
    }
    rfd::FileDialog::new().set_title("Pick a front cover").add_filter("Images", &["jpg", "jpeg", "png", "webp"]).pick_file()
}

// ---- Saving -------------------------------------------------------------------------------

/// Where a save is, as the panel shows it.
#[derive(Clone, Debug)]
pub enum Saving {
    Progress { done: usize, total: usize, name: String },
    Done(Box<Outcome>),
}

/// How a save went, as the panel tells it.
#[derive(Clone, Debug, Default)]
pub struct Outcome {
    pub report: edit::Report,
    pub total: usize,
    /// The journal entry (to undo what was written).
    pub id: Option<String>,
    pub undo: bool,
}

type Handler = Rc<dyn Fn(&Rc<State>, Saving)>;

thread_local! {
    /// The save under way (its Cancel), if any.
    static CANCEL: RefCell<Option<Arc<AtomicBool>>> = const { RefCell::new(None) };
    /// Who hears about it (the panel).
    static HANDLER: RefCell<Option<Handler>> = const { RefCell::new(None) };
}

fn deliver(st: &Rc<State>, msg: Saving) {
    let done = matches!(msg, Saving::Done(_));
    let h = HANDLER.with(|h| h.borrow().clone());
    if done {
        CANCEL.with(|c| c.borrow_mut().take());
        HANDLER.with(|h| h.borrow_mut().take());
    }
    if let Some(h) = h {
        h(st, msg);
    }
}

pub fn saving() -> bool {
    CANCEL.with(|c| c.borrow().is_some())
}

/// Stop after the file being written.
pub fn cancel() {
    CANCEL.with(|c| {
        if let Some(f) = c.borrow().as_ref() {
            f.store(true, Ordering::SeqCst);
        }
    });
}

/// Write `edits` on a thread of their own; `on` hears the progress and the end (on the
/// UI thread). Refused (false) with tag editing off, or while another save runs.
pub fn save(st: &Rc<State>, edits: Vec<FileEdit>, label: String, undoes: Option<String>, on: impl Fn(&Rc<State>, Saving) + 'static) -> bool {
    if !enabled(st) || saving() || edits.is_empty() {
        return false;
    }
    let cancel = Arc::new(AtomicBool::new(false));
    CANCEL.with(|c| *c.borrow_mut() = Some(cancel.clone()));
    let j = journal(st);
    let total = edits.len();
    let undo = undoes.is_some();
    HANDLER.with(|h| *h.borrow_mut() = Some(Rc::new(on)));
    let spawned = std::thread::Builder::new().name("mp-tag-save".into()).spawn(move || {
        library::background_thread();
        let progress = |p: edit::Progress| {
            let name = p.current.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let msg = Saving::Progress { done: p.done, total: p.total, name };
            state::post(move |st| deliver(st, msg));
        };
        let report = edit::save_all(&edits, &j, &label, undoes, &cancel, &progress);
        let rid = report.id.clone();
        let msg = Saving::Done(Box::new(Outcome { report, total, id: rid, undo }));
        state::post(move |st| deliver(st, msg));
    });
    if let Err(e) = spawned {
        log::error!("tag editor: cannot start the save thread: {e}");
        CANCEL.with(|c| c.borrow_mut().take());
        HANDLER.with(|h| h.borrow_mut().take());
        return false;
    }
    true
}

/// After a save: the library takes the files' new tags as from any retag, scanning where
/// they are (what the folder watcher would also ask for; the second ask finds nothing to
/// read).
pub fn rescan_saved(st: &Rc<State>, saved: &[PathBuf]) {
    if saved.is_empty() || st.settings.borrow().folders.is_empty() {
        return;
    }
    crate::scanning::rescan_changed(st, library::watch::Change::Paths(saved.to_vec()));
}

/// "Saved 12 files." / "Saved 6 of 12 files. …", for the panel.
pub fn outcome_text(o: &Outcome) -> (String, String, bool) {
    let r = &o.report;
    let n = |k: usize| if k == 1 { "1 file".to_owned() } else { format!("{k} files") };
    let name = |p: &Path| p.file_name().map(|x| x.to_string_lossy().into_owned()).unwrap_or_default();
    let verb = if o.undo { "Undone" } else { "Saved" };
    let written = r.saved.len();
    let mut lines = Vec::new();
    if !r.unchanged.is_empty() {
        lines.push(format!("{} already held these values and {} not rewritten.", n(r.unchanged.len()), if r.unchanged.len() == 1 { "was" } else { "were" }));
    }
    if let Some((p, why)) = &r.failed {
        lines.push(format!("{} could not be {}: {why}. It is as it was.", name(p), if o.undo { "undone" } else { "saved" }));
    }
    if !r.not_tried.is_empty() {
        let list: Vec<String> = r.not_tried.iter().take(8).map(|p| name(p)).collect();
        let more = if r.not_tried.len() > 8 { format!(", and {} more", r.not_tried.len() - 8) } else { String::new() };
        let verb = if o.undo { "undone" } else { "written" };
        lines.push(format!("{} {} {} not {verb} ({}{more}).", if r.cancelled { "Cancelled:" } else { "Stopped there:" }, n(r.not_tried.len()), if r.not_tried.len() == 1 { "was" } else { "were" }, list.join(", ")));
    }
    let notes: Vec<&String> = {
        let mut v: Vec<&String> = r.saved.iter().flat_map(|s| s.notes.iter()).collect();
        v.sort();
        v.dedup();
        v
    };
    for note in notes.into_iter().take(4) {
        lines.push(format!("{note}."));
    }
    let ok = r.failed.is_none() && !r.cancelled;
    let title = if ok {
        format!("{verb} {}.", n(written + r.unchanged.len()))
    } else if written == 0 {
        if o.undo { "Nothing was undone.".into() } else { "Nothing was saved.".into() }
    } else {
        format!("{verb} {} of {}.", written, n(o.total))
    };
    (title, lines.join(" "), !ok)
}

/// One file's path twice: letter case aside on Windows and macOS, whose file systems ignore it
/// (APFS and HFS+ as macOS formats them); exactly on Linux, where `Track.flac` and
/// `track.flac` are two files.
fn same_file(a: &str, b: &str) -> bool {
    if cfg!(any(windows, target_os = "macos")) { a.eq_ignore_ascii_case(b) } else { a == b }
}

/// Every save in the journal that wrote one of these files, newest first: (save, the
/// number of these files it wrote, undone already).
pub fn history_for(j: &Journal, paths: &[String]) -> Vec<(edit::journal::Save, usize, bool)> {
    let all = j.list();
    let undone: Vec<String> = all.iter().filter(|s| !s.files.is_empty()).filter_map(|s| s.header.undoes.clone()).collect();
    let mut out: Vec<_> = all
        .into_iter()
        .filter_map(|s| {
            let n = s.files.iter().filter(|f| paths.iter().any(|p| same_file(p, &f.path))).count();
            (n > 0).then(|| {
                let done = undone.contains(&s.header.id);
                (s, n, done)
            })
        })
        .collect();
    out.reverse();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_draft_keeps_what_was_and_forgets_a_change_back() {
        let mut d = Draft::default();
        d.set("a.flac", "a.flac", None, "TITLE", vec!["Old".into()], vec!["New".into()]);
        d.set("a.flac", "a.flac", None, "TITLE", vec!["ignored".into()], vec!["Newer".into()]);
        assert_eq!(d.field("a.flac", "TITLE"), Some(&Change { old: vec!["Old".into()], new: vec!["Newer".into()] }), "the first old value stays");
        d.set("b.flac", "b.flac", None, "ALBUM", vec![], vec!["X".into()]);
        assert_eq!(d.status(), "2 unsaved changes in 2 files");
        assert_eq!(Draft::lines(d.file("b.flac").unwrap()), ["Album: \u{201c}X\u{201d} (new)"]);
        d.set("a.flac", "a.flac", None, "TITLE", vec![], vec!["Old".into()]);
        assert!(d.file("a.flac").is_none(), "back to what it was: no change");
        assert_eq!(d.status(), "1 unsaved change");
        let e = d.edits();
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].fields, [("ALBUM".to_owned(), vec!["X".to_owned()])]);
        d.set_cover("b.flac", "b.flac", None, Some(Cover::Remove));
        assert_eq!(d.label("Album \u{201c}P\u{201d}"), "Album \u{201c}P\u{201d}: Album and 1 more");
    }

    #[test]
    fn a_file_is_the_same_by_letter_case_only_on_windows() {
        assert!(same_file("/m/a/Track.flac", "/m/a/Track.flac"));
        assert!(!same_file("/m/a/Track.flac", "/m/a/Other.flac"));
        assert_eq!(same_file("/m/a/Track.flac", "/m/a/track.flac"), cfg!(any(windows, target_os = "macos")));
    }

    #[test]
    fn typed_values_are_read_forgivingly() {
        assert_eq!(read_values("TRACKNUMBER", &["3/12".into()]), Ok(vec![("TRACKNUMBER".into(), vec!["3".into()]), ("TRACKTOTAL".into(), vec!["12".into()])]));
        assert_eq!(read_values("DATE", &["2019.3.8".into()]).unwrap()[0].1, ["2019-03-08"]);
        assert!(read_values("TRACKNUMBER", &["A1".into()]).is_err());
        assert_eq!(read_values("ARTIST", &["A".into(), "".into(), "B".into()]).unwrap()[0].1, ["A", "B"]);
    }

    #[test]
    fn numbers_and_totals_come_out_of_one_item() {
        use library::meta::{BlockKind, FileTags, RawItem};
        let tags = FileTags { blocks: vec![BlockKind::Id3v2(4)], items: vec![RawItem { block: 0, key: "TRCK".into(), values: vec!["3/12".into()], ..Default::default() }], ..Default::default() };
        let f = meta::fields(&tags);
        assert_eq!(values_of(&f, "TRACKNUMBER"), ["3"]);
        assert_eq!(values_of(&f, "TRACKTOTAL"), ["12"]);
        assert!(values_of(&f, "DISCNUMBER").is_empty());
    }
}
