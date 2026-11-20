//! `MP_BENCH=output-default` (Linux): the system default device as the equalizer, Settings >
//! Output and PipeWire itself see it, when the default changes while the app runs: at start,
//! playing, paused and stopped, the default is moved between the headless session's two null
//! sinks with `wpctl set-default`, and each time the three must agree: the equalizer's device
//! (`Equalizer.device`), Settings > Output (the menu's "System default (…)" and "Playing on …"),
//! and the sink the stream is really linked to (`pw-dump`: the links from the app's node).
//!
//! It changes the default only between "Test Speakers" and "Test Headphones"
//! (`packaging/linux/headless-session.sh`), and puts the first default back at the end; where
//! those two sinks are not there (a real desktop) it checks the start only and changes nothing.
//! Nothing is audible (`MP_VOLUME=0`, null sinks).

use std::cell::RefCell;
use std::rc::Rc;

use library::query;
use slint::ComponentHandle;

use crate::bench::{Step, run_steps, verdict, write_report};
use crate::state::State;
use crate::{AppWindow, Equalizer, Output, playback};

#[derive(Default)]
struct Ctx {
    r: String,
    /// The default at the start (`node.name`), put back at the end.
    first: Option<String>,
    /// The two test sinks (`node.name`, PipeWire id), when the session has them.
    test: Option<[(String, u32); 2]>,
}

type C = Rc<RefCell<Ctx>>;

fn line(c: &C, text: String) {
    log::info!("output-default bench: {text}");
    let mut c = c.borrow_mut();
    c.r.push_str(&text);
    c.r.push('\n');
}

fn check(c: &C, what: &str, ok: bool, detail: impl std::fmt::Display) {
    verdict(&mut c.borrow_mut().r, what, ok, detail);
}

/// PipeWire's objects (`pw-dump`), or nothing without it.
fn dump() -> Vec<serde_json::Value> {
    let out = std::process::Command::new("pw-dump").arg("--no-colors").stderr(std::process::Stdio::null()).output();
    out.ok().and_then(|o| serde_json::from_slice::<Vec<serde_json::Value>>(&o.stdout).ok()).unwrap_or_default()
}

fn props(o: &serde_json::Value) -> &serde_json::Value {
    &o["info"]["props"]
}

/// The default sink the session manager says is in effect (`default.audio.sink`).
fn pw_default(objects: &[serde_json::Value]) -> Option<String> {
    // (A metadata object carries its properties at the top, not under `info`.)
    objects.iter().filter(|o| o["type"] == "PipeWire:Interface:Metadata" && o["props"]["metadata.name"] == "default").find_map(|o| {
        o["metadata"].as_array()?.iter().find(|m| m["key"] == "default.audio.sink").and_then(|m| m["value"]["name"].as_str().map(str::to_owned))
    })
}

/// The sinks the app's playback stream is linked to (`node.name`s).
fn pw_linked(objects: &[serde_json::Value]) -> Vec<String> {
    let id_of = |o: &serde_json::Value| o["id"].as_u64();
    let ours: Vec<u64> = objects
        .iter()
        .filter(|o| o["type"] == "PipeWire:Interface:Node" && props(o)["media.class"] == "Stream/Output/Audio" && props(o)["node.name"] == crate::platform::APP_ID)
        .filter_map(id_of)
        .collect();
    let mut names: Vec<String> = objects
        .iter()
        .filter(|o| o["type"] == "PipeWire:Interface:Link")
        .filter(|l| l["info"]["output-node-id"].as_u64().is_some_and(|n| ours.contains(&n)))
        .filter_map(|l| {
            let to = l["info"]["input-node-id"].as_u64()?;
            objects.iter().find(|o| id_of(o) == Some(to)).and_then(|o| props(o)["node.name"].as_str().map(str::to_owned))
        })
        .collect();
    names.sort();
    names.dedup();
    names
}

fn sink_id(objects: &[serde_json::Value], name: &str) -> Option<u32> {
    objects.iter().find(|o| o["type"] == "PipeWire:Interface:Node" && props(o)["node.name"] == name).and_then(|o| o["id"].as_u64()).map(|i| i as u32)
}

fn set_default(c: &C, id: u32, name: &str) {
    let ok = std::process::Command::new("wpctl").args(["set-default", &id.to_string()]).status().is_ok_and(|s| s.success());
    line(c, format!("wpctl set-default {id} ({name}): {}", if ok { "done" } else { "FAILED" }));
}

/// The three views of the default, compared with what PipeWire says now. `playing`: the
/// stream exists and must be linked to it.
fn agree(c: &C, st: &Rc<State>, app: &AppWindow, when: &str, stream: bool) {
    let objects = dump();
    let Some(want) = pw_default(&objects) else {
        check(c, &format!("{when}: PipeWire names a default"), false, "pw-dump gave none");
        return;
    };
    let devices = crate::output::devices().unwrap_or_default();
    let name_of = |id: &str| devices.iter().find(|d| d.id == id).map(|d| d.name.clone()).unwrap_or_else(|| id.to_owned());
    let want_name = name_of(&want);
    let listed = devices.iter().find(|d| d.default).map(|d| d.id.clone());
    check(c, &format!("{when}: Settings > Output's list marks the default PipeWire says"), listed.as_deref() == Some(want.as_str()), format!("listed {listed:?}, PipeWire {want}"));
    let eq = app.global::<Equalizer>().get_device().to_string();
    check(c, &format!("{when}: the equalizer names the default"), eq == want_name, format!("equalizer {eq:?}, default {want_name:?}"));
    let id = crate::eq::device_id(&st.settings.borrow());
    check(c, &format!("{when}: the equalizer's setting is the default's"), id.as_deref() == Some(want.as_str()), format!("{id:?}"));
    if stream {
        let linked = pw_linked(&objects);
        check(c, &format!("{when}: the stream is linked to the default"), linked == [want.clone()], format!("{linked:?}"));
        let status = app.global::<Output>().get_status().to_string();
        check(c, &format!("{when}: Settings > Output says it plays there"), status.starts_with(&format!("Playing on {want_name}")), status);
        let info = crate::output::info().map(|i| (i.id, i.device));
        check(c, &format!("{when}: the engine reports it"), info.as_ref().is_some_and(|(i, _)| i == &want), format!("{info:?}"));
    }
}

pub fn run() {
    let c: C = Rc::default();
    let mut steps: Vec<Step> = Vec::new();
    macro_rules! step {
        ($ms:expr, |$st:ident, $app:ident, $c:ident| $body:block) => {{
            let $c = c.clone();
            steps.push(($ms, Box::new(move |$st: &Rc<State>, $app: &AppWindow| $body)));
        }};
    }

    if !cfg!(target_os = "linux") {
        step!(500, |_st, _app, c| {
            line(&c, "Linux only (PipeWire's default): nothing to check here".into());
            write_report("output-default", &c.borrow().r);
            let _ = slint::quit_event_loop();
        });
        run_steps(steps);
        return;
    }
    step!(1500, |st, app, c| {
        let objects = dump();
        let first = pw_default(&objects);
        line(&c, format!("PipeWire's default at the start: {first:?}; {} objects", objects.len()));
        let test = ["mp-null-a", "mp-null-b"].map(|n| sink_id(&objects, n).map(|i| (n.to_owned(), i)));
        if let [Some(a), Some(b)] = test {
            c.borrow_mut().test = Some([a, b]);
        } else {
            line(&c, "not the headless session's sinks: the default is not changed".into());
        }
        c.borrow_mut().first = first;
        agree(&c, st, app, "at start, nothing played", false);
    });
    // While nothing has played: the default moves, the equalizer follows.
    step!(200, |_st, _app, c| {
        let other = { c.borrow().test.clone() }.and_then(|t| t.into_iter().find(|s| Some(&s.0) != c.borrow().first.as_ref()));
        if let Some((name, id)) = other {
            set_default(&c, id, &name);
        }
    });
    step!(1500, |st, app, c| {
        if c.borrow().test.is_some() {
            agree(&c, st, app, "nothing played, the default changed", false);
        }
        let albums = query::all_albums(&st.db, query::AlbumSort::Artist, false).unwrap_or_default();
        if let Some(a) = albums.first() {
            playback::play_tracks(st, query::album_tracks(&st.db, a.id).unwrap_or_default(), 0);
        }
    });
    step!(2500, |st, app, c| {
        agree(&c, st, app, "playing", true);
        let next = { c.borrow().test.clone() }.and_then(|t| t.into_iter().find(|s| pw_default(&dump()).as_ref() != Some(&s.0)));
        if let Some((name, id)) = next {
            set_default(&c, id, &name);
        }
    });
    step!(2000, |st, app, c| {
        if c.borrow().test.is_some() {
            agree(&c, st, app, "playing, the default changed", true);
        }
        playback::toggle_play(st);
    });
    step!(800, |_st, _app, c| {
        let next = { c.borrow().test.clone() }.and_then(|t| t.into_iter().find(|s| pw_default(&dump()).as_ref() != Some(&s.0)));
        if let Some((name, id)) = next {
            set_default(&c, id, &name);
        }
    });
    step!(2000, |st, app, c| {
        if c.borrow().test.is_some() {
            agree(&c, st, app, "paused, the default changed", true);
        }
        st.player.stop();
    });
    step!(800, |_st, _app, c| {
        let next = { c.borrow().test.clone() }.and_then(|t| t.into_iter().find(|s| pw_default(&dump()).as_ref() != Some(&s.0)));
        if let Some((name, id)) = next {
            set_default(&c, id, &name);
        }
    });
    step!(2000, |st, app, c| {
        if c.borrow().test.is_some() {
            agree(&c, st, app, "stopped, the default changed", false);
        }
        // Put the first default back.
        let first = c.borrow().first.clone();
        let objects = dump();
        if c.borrow().test.is_some()
            && let Some(name) = first
            && let Some(id) = sink_id(&objects, &name)
        {
            set_default(&c, id, &name);
        }
    });
    step!(500, |_st, _app, c| {
        write_report("output-default", &c.borrow().r);
        let _ = slint::quit_event_loop();
    });
    run_steps(steps);
}
