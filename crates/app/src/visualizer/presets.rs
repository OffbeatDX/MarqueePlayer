//! The visualizer's presets the user makes (Settings > Visualizer, the preset menu): save
//! the look under a name, rename, update to the look now, delete; and share one as text
//! (copy, paste: one line a chat message carries, `visualizer/share.rs`) or as a file (export,
//! import). A pasted or imported preset is shown in the preview first and added only when
//! the user says so (`Trial`): its own name, or Replace / Keep both when the name is taken.
//! A preset is a look only: not where it shows, not whether it is on. They are kept in
//! settings.json, `visualizer_presets` (`look::UserPresets`).

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use slint::ComponentHandle;

use crate::state::State;
use crate::visualizer::look::{self, Look, Pick, Preset, UserLook};
use crate::visualizer::share::{self, Refusal};
use crate::{Dialog, MenuStore, Visualizer, views};

/// What the name prompt is asking for.
#[derive(Clone, Debug, PartialEq)]
enum PromptFor {
    /// Save the look as a preset; `replace`: the user was told this name is taken and may
    /// press again to replace it.
    Save { replace: Option<String> },
    Rename(String),
}

/// A shared preset shown in the preview, waiting for the user's yes.
#[derive(Clone, Debug, PartialEq)]
pub struct Trial {
    pub look: Look,
    /// The name it would have (its own, or a made-up one).
    pub name: String,
    /// One of the user's presets has the name (as written there); `identical`: and the look.
    pub taken: Option<String>,
    pub identical: bool,
    /// Where it came from: "the clipboard", or the file's name.
    pub from: String,
    pub notes: Vec<String>,
}

thread_local! {
    static PROMPT: RefCell<Option<PromptFor>> = const { RefCell::new(None) };
    static TRIAL: RefCell<Option<Trial>> = const { RefCell::new(None) };
}

fn presets(st: &State) -> look::UserPresets {
    st.settings.borrow().visualizer_presets.clone()
}

fn save(st: &Rc<State>, p: look::UserPresets) {
    st.settings.borrow_mut().visualizer_presets = p;
    st.save_settings_soon();
    crate::visualizer::publish(st);
}

/// The user's preset the look is now, or was chosen from and has changed since: (name,
/// whether it still is that look).
pub fn current_user(st: &State) -> Option<(String, bool)> {
    let app = st.app.upgrade()?;
    let s = crate::visualizer::settings(st);
    let us = crate::visualizer::users(st);
    match s.current(&crate::visualizer::theme(&app), &us) {
        Some(Pick::User(i)) => Some((us[i].name.clone(), true)),
        Some(Pick::BuiltIn(_)) => None,
        None => s.based_on.filter(|n| us.iter().any(|u| &u.name == n)).map(|n| (n, false)),
    }
}

/// The look in effect, and the name it goes by ("Classic", the user's, "Custom").
fn look_now(st: &State) -> Option<(String, Look)> {
    let app = st.app.upgrade()?;
    let s = crate::visualizer::settings(st);
    let t = crate::visualizer::theme(&app);
    let us = crate::visualizer::users(st);
    let name = match s.current(&t, &us) {
        Some(Pick::BuiltIn(p)) => p.label().to_owned(),
        Some(Pick::User(i)) => us[i].name.clone(),
        None => "Custom".to_owned(),
    };
    Some((name, s.effective(&t)))
}

// ---- The menu -------------------------------------------------------------------------------

/// Settings > Visualizer's preset menu: what can be done with the presets.
pub fn menu_entries(st: &State) -> Vec<crate::menus::Entry> {
    use crate::menus::{Act, Entry};
    let mut v = vec![Entry::Item { label: "Save as preset\u{2026}".into(), act: Act::VizSave }];
    if let Some((name, same)) = current_user(st) {
        if !same {
            v.push(Entry::Item { label: format!("Update \u{201c}{name}\u{201d} to this look"), act: Act::VizUpdate });
        }
        v.push(Entry::Item { label: format!("Rename \u{201c}{name}\u{201d}\u{2026}"), act: Act::VizRename });
        v.push(Entry::Item { label: format!("Delete \u{201c}{name}\u{201d}"), act: Act::VizDelete });
    }
    v.push(Entry::Separator);
    v.push(Entry::Item { label: "Copy this look as text".into(), act: Act::VizCopy });
    v.push(Entry::Item { label: "Paste a preset\u{2026}".into(), act: Act::VizPaste });
    v.push(Entry::Separator);
    v.push(Entry::Item { label: "Export as a file\u{2026}".into(), act: Act::VizExport });
    v.push(Entry::Item { label: "Import a file\u{2026}".into(), act: Act::VizImport });
    v
}

pub fn open_menu(st: &Rc<State>, x: f32, y: f32) {
    let e = menu_entries(st);
    crate::menus::show_entries(st, e, x, y);
}

// ---- Save as, rename, update, delete --------------------------------------------------------

fn open_prompt(st: &Rc<State>, purpose: PromptFor, title: &str, prefill: &str, confirm: &str) {
    let Some(app) = st.app.upgrade() else { return };
    let d = app.global::<Dialog>();
    if d.get_open() {
        return;
    }
    let (x, y) = (app.global::<MenuStore>().get_x(), app.global::<MenuStore>().get_y());
    PROMPT.with(|p| *p.borrow_mut() = Some(purpose));
    d.set_prompt_title(title.into());
    d.set_prompt_text(prefill.into());
    d.set_prompt_note("".into());
    d.set_prompt_confirm(confirm.into());
    d.set_prompt_placeholder("Preset name".into());
    d.set_x(x);
    d.set_y(y);
    d.set_returns(false);
    d.set_prompt_open(true);
}

/// "Save as preset…": asks for a name (the user's preset it came from, or a new one).
pub fn save_prompt(st: &Rc<State>) {
    let prefill = match (current_user(st), look_now(st)) {
        (Some((name, _)), _) => name,
        (None, Some((name, _))) if name != "Custom" => format!("{name} (mine)"),
        _ => "My look".to_owned(),
    };
    open_prompt(st, PromptFor::Save { replace: None }, "Save the look as a preset", &prefill, "Save");
}

pub fn rename_prompt(st: &Rc<State>) {
    let Some((name, _)) = current_user(st) else { return };
    open_prompt(st, PromptFor::Rename(name.clone()), "Rename preset", &name, "Rename");
}

/// The name prompt is for a visualizer preset.
pub fn prompt_active() -> bool {
    PROMPT.with(|p| p.borrow().is_some())
}

pub fn prompt_edited(st: &Rc<State>, text: &str) {
    let offer = PROMPT.with(|p| matches!(&*p.borrow(), Some(PromptFor::Save { replace: Some(n) }) if !look::same_name(n, text)));
    if offer {
        PROMPT.with(|p| *p.borrow_mut() = Some(PromptFor::Save { replace: None }));
    }
    if let Some(app) = st.app.upgrade() {
        let d = app.global::<Dialog>();
        if offer || d.get_prompt_note() != "" {
            d.set_prompt_note("".into());
            d.set_prompt_confirm(if matches!(PROMPT.with(|p| p.borrow().clone()), Some(PromptFor::Rename(_))) { "Rename" } else { "Save" }.into());
        }
    }
}

pub fn prompt_confirm(st: &Rc<State>, text: &str) {
    let Some(app) = st.app.upgrade() else { return };
    let d = app.global::<Dialog>();
    let Some(purpose) = PROMPT.with(|p| p.borrow().clone()) else { return };
    let name = look::clean_name(text);
    let note = |t: &str| d.set_prompt_note(t.into());
    let p = presets(st);
    if name.is_empty() {
        note("Type a name for the preset.");
        return;
    }
    if let Some(b) = Preset::ALL.iter().find(|b| look::same_name(b.label(), &name)) {
        note(&format!("\u{201c}{}\u{201d} is a built-in preset. Type another name.", b.label()));
        return;
    }
    let taken = p.find(&name).map(|i| p.list[i].name.clone());
    match (&purpose, taken) {
        (PromptFor::Rename(old), Some(n)) if !look::same_name(old, &n) => note(&format!("Another preset is called \u{201c}{n}\u{201d}. Type another name.")),
        (PromptFor::Save { replace }, Some(n)) if replace.as_deref() != Some(n.as_str()) => {
            PROMPT.with(|p| *p.borrow_mut() = Some(PromptFor::Save { replace: Some(n.clone()) }));
            note(&format!("There is a preset called \u{201c}{n}\u{201d} already. Replace it, or type another name."));
            d.set_prompt_confirm("Replace".into());
        }
        (PromptFor::Save { .. }, _) => {
            close_prompt(st);
            save_as(st, &name);
        }
        (PromptFor::Rename(old), _) => {
            let old = old.clone();
            close_prompt(st);
            rename(st, &old, &name);
        }
    }
}

pub fn prompt_cancel(st: &Rc<State>) {
    close_prompt(st);
}

fn close_prompt(st: &Rc<State>) {
    PROMPT.with(|p| *p.borrow_mut() = None);
    if let Some(app) = st.app.upgrade() {
        let d = app.global::<Dialog>();
        d.set_prompt_open(false);
        d.set_prompt_placeholder("Playlist name".into());
    }
}

/// Keep the look as a preset called `name` (replacing one of that name), and make it the
/// one in use.
pub fn save_as(st: &Rc<State>, name: &str) {
    let Some((_, look)) = look_now(st) else { return };
    add(st, name, look, true);
    views::flash(st, &format!("Saved \u{201c}{name}\u{201d}"));
}

/// Add (or replace) a preset and choose it.
fn add(st: &Rc<State>, name: &str, look: Look, replace: bool) {
    let mut p = presets(st);
    let u = UserLook { name: name.to_owned(), look: look.clamped() };
    match p.find(name).filter(|_| replace) {
        Some(i) => p.list[i] = u.clone(),
        None => p.list.push(u.clone()),
    }
    save(st, p);
    crate::visualizer::update(st, |s, _| s.choose_user(&u));
}

pub fn rename(st: &Rc<State>, old: &str, new: &str) {
    let mut p = presets(st);
    let Some(i) = p.list.iter().position(|u| u.name == old) else { return };
    p.list[i].name = new.to_owned();
    save(st, p);
    let renamed = crate::visualizer::settings(st).based_on.as_deref() == Some(old);
    if renamed {
        crate::visualizer::update(st, |s, _| s.based_on = Some(new.to_owned()));
    }
    views::flash(st, &format!("Renamed to \u{201c}{new}\u{201d}"));
}

/// "Update": the user's preset the look came from takes the look as it is now.
pub fn update(st: &Rc<State>) {
    let (Some((name, _)), Some((_, look))) = (current_user(st), look_now(st)) else { return };
    add(st, &name, look, true);
    views::flash(st, &format!("Updated \u{201c}{name}\u{201d}"));
}

/// Delete the user's preset the look is (or came from). The look stays, as Custom.
pub fn delete(st: &Rc<State>) {
    let Some((name, _)) = current_user(st) else { return };
    let mut p = presets(st);
    p.list.retain(|u| u.name != name);
    save(st, p);
    crate::visualizer::update(st, |s, _| s.based_on = None);
    views::flash(st, &format!("Deleted \u{201c}{name}\u{201d}"));
}

// ---- Copy and paste --------------------------------------------------------------------------

/// The look in effect as the text Copy puts on the clipboard.
pub fn copy_text(st: &State) -> Option<String> {
    let (name, look) = look_now(st)?;
    Some(share::to_text(&name, &look))
}

/// Copy (the menu, Ctrl+C on the page): the look on the clipboard as one line of text.
pub fn copy(st: &Rc<State>) {
    let Some(text) = copy_text(st) else { return };
    if crate::explorer::copy_text(&text) {
        views::flash(st, "Visualizer look copied: paste it in a message, or in Marquee Player");
    } else {
        views::flash(st, "Could not use the clipboard");
    }
}

/// Paste (the menu, Ctrl+V on the page): a preset from the clipboard, shown in the preview.
pub fn paste(st: &Rc<State>) {
    match crate::explorer::paste_text() {
        Err(e) => views::flash_long(st, &e),
        Ok(None) => views::flash_long(st, "Nothing to paste: the clipboard has no text"),
        Ok(Some(t)) if t.trim().is_empty() => views::flash_long(st, "Nothing to paste: the clipboard has no text"),
        Ok(Some(t)) => read(st, &t, None),
    }
}

/// Read a preset file and show it in the preview (nothing is added until the user says so).
/// Errors that stop it before reading (missing, far too large) are returned.
pub fn import_file(st: &Rc<State>, path: &Path) -> Result<(), String> {
    let file = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let size = std::fs::metadata(path).map_err(|e| format!("Could not read {}: {e}", path.display()))?.len();
    if size > (share::MAX_BYTES * 4) as u64 {
        return Err(format!("\u{201c}{file}\u{201d} is too large to be a visualizer preset ({:.1} MB)", size as f64 / 1_048_576.0));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("Could not read {}: {e}", path.display()))?;
    let text = String::from_utf8_lossy(&bytes);
    read(st, &text, Some(file));
    Ok(())
}

fn read(st: &Rc<State>, text: &str, file: Option<String>) {
    match share::parse(text) {
        Err(Refusal::NotAPreset) => views::flash_long(
            st,
            &match &file {
                Some(f) => format!("\u{201c}{f}\u{201d} is not a visualizer preset"),
                None => "Nothing to paste: no visualizer preset on the clipboard".to_owned(),
            },
        ),
        Err(Refusal::Unreadable(why)) => views::flash_long(
            st,
            &match &file {
                Some(f) => format!("Cannot import \u{201c}{f}\u{201d}: {why}"),
                None => format!("Cannot paste this preset: {why}"),
            },
        ),
        Ok(shared) => {
            // Shown in Settings > Visualizer's preview: go there if pasted from elsewhere.
            if !crate::visualizer::shown(st) {
                views::settings_section(st, crate::settings::nav::VISUALIZER);
            }
            start_trial(st, shared, file);
        }
    }
}

fn start_trial(st: &Rc<State>, shared: share::Shared, file: Option<String>) {
    let Some(app) = st.app.upgrade() else { return };
    let p = presets(st);
    let base = shared.name.clone().unwrap_or_else(|| if file.is_some() { "Imported look".to_owned() } else { "Pasted look".to_owned() });
    // A built-in preset's name is never taken over: the shared one is kept beside it.
    let base = if Preset::ALL.iter().any(|b| look::same_name(b.label(), &base)) { look::unique_name(&base, |n| p.taken(n)) } else { base };
    let taken = p.find(&base).map(|i| p.list[i].name.clone());
    let identical = p.find(&base).is_some_and(|i| p.list[i].look.same(&shared.look));
    let trial = Trial { look: shared.look, name: base, taken, identical, from: file.clone().map_or_else(|| "the clipboard".to_owned(), |f| format!("\u{201c}{f}\u{201d}")), notes: shared.notes };
    let g = app.global::<Visualizer>();
    g.set_trial(true);
    g.set_trial_name(trial.name.clone().into());
    g.set_trial_taken(trial.taken.is_some() && !trial.identical);
    g.set_trial_identical(trial.identical);
    g.set_trial_text(trial_words(&trial).into());
    g.set_trial_shown(g.get_trial_shown().wrapping_add(1));
    TRIAL.with(|t| *t.borrow_mut() = Some(trial));
    crate::visualizer::publish(st);
    crate::visualizer::restyle(st);
}

/// What the preview says about a preset waiting there.
fn trial_words(t: &Trial) -> String {
    let what = if t.identical {
        format!("\u{201c}{}\u{201d} from {}: you have this preset already", t.name, t.from)
    } else if t.taken.is_some() {
        format!("\u{201c}{}\u{201d} from {}: you have a preset of that name. Replace it, or keep both?", t.name, t.from)
    } else {
        format!("\u{201c}{}\u{201d} from {}, shown here: add it?", t.name, t.from)
    };
    if t.notes.is_empty() { what } else { format!("{what} ({})", t.notes.join("; ")) }
}

/// The look a preset waiting in the preview has (the preview draws it).
pub fn trial_look() -> Option<Look> {
    TRIAL.with(|t| t.borrow().as_ref().map(|t| t.look.clone()))
}

#[cfg(feature = "bench")]
pub fn trial() -> Option<Trial> {
    TRIAL.with(|t| t.borrow().clone())
}

/// The preview's answer: "add" (or "replace" a preset of that name), "keep" (both: the new
/// one under a free name), "use" (the identical one already there), or "cancel".
pub fn trial_answer(st: &Rc<State>, answer: &str) {
    let Some(t) = TRIAL.with(|t| t.borrow_mut().take()) else { return };
    clear_trial_ui(st);
    match answer {
        "add" | "replace" => {
            add(st, &t.name, t.look, true);
            views::flash(st, &if t.taken.is_some() && !t.identical { format!("Replaced \u{201c}{}\u{201d}", t.name) } else { format!("Added \u{201c}{}\u{201d}", t.name) });
        }
        "keep" => {
            let p = presets(st);
            let name = look::unique_name(&t.name, |n| p.taken(n));
            add(st, &name, t.look, false);
            views::flash(st, &format!("Added \u{201c}{name}\u{201d}"));
        }
        "use" => {
            if let Some(u) = presets(st).find(&t.name).map(|i| presets(st).list[i].clone()) {
                crate::visualizer::update(st, |s, _| s.choose_user(&u));
            }
        }
        _ => {
            crate::visualizer::publish(st);
            crate::visualizer::restyle(st);
        }
    }
}

fn clear_trial_ui(st: &Rc<State>) {
    if let Some(app) = st.app.upgrade() {
        app.global::<Visualizer>().set_trial(false);
    }
}

/// A preset waiting in the preview gives way (the user changed the look, left the section).
pub fn drop_trial(st: &Rc<State>) {
    if TRIAL.with(|t| t.borrow_mut().take()).is_some() {
        clear_trial_ui(st);
        crate::visualizer::publish(st);
        crate::visualizer::restyle(st);
    }
}

// ---- Files ----------------------------------------------------------------------------------

/// A file name for a preset's name (what Windows and Linux refuse in a name replaced).
pub fn file_name(name: &str) -> String {
    let base: String = name.chars().map(|c| if r#"\/:*?"<>|"#.contains(c) || c.is_control() { '_' } else { c }).collect();
    let base = base.trim().trim_end_matches('.').to_owned();
    format!("{}.mpviz", if base.is_empty() { "Visualizer look".to_owned() } else { base })
}

pub fn export_file(st: &Rc<State>, path: &Path) -> bool {
    let Some((name, look)) = look_now(st) else { return false };
    match std::fs::write(path, share::to_file(&name, &look)) {
        Ok(()) => {
            views::flash(st, &format!("Exported to {}", path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default()));
            true
        }
        Err(e) => {
            views::flash_long(st, &format!("Could not write {}: {e}", path.display()));
            false
        }
    }
}

pub fn export_dialog(st: &Rc<State>) {
    if !crate::dialog_allowed("export visualizer preset") {
        return;
    }
    let Some((name, _)) = look_now(st) else { return };
    let dialog = rfd::FileDialog::new().set_title("Export the visualizer's look").set_file_name(file_name(&name)).add_filter("Marquee Player visualizer preset", &["mpviz"]);
    if let Some(path) = dialog.save_file() {
        export_file(st, &path);
    }
}

pub fn import_dialog(st: &Rc<State>) {
    if !crate::dialog_allowed("import visualizer preset") {
        return;
    }
    let picked = rfd::FileDialog::new().set_title("Import a visualizer preset").add_filter("Visualizer preset", &["mpviz", "txt"]).add_filter("All files", &["*"]).pick_file();
    if let Some(path) = picked
        && let Err(e) = import_file(st, &path)
    {
        views::flash_long(st, &e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names_are_safe() {
        assert_eq!(file_name("Night halo"), "Night halo.mpviz");
        assert_eq!(file_name("a/b:c*?"), "a_b_c__.mpviz");
        assert_eq!(file_name("  ...  "), "Visualizer look.mpviz");
    }
}
