//! `MP_BENCH=xfade-rates`: a crossfade between tracks at different sample rates in the real app
//! (`bench-xfade-rates.txt`, `xr-*.png`), on the technical details' fixtures (`formats`): the
//! 48 kHz Opus track of "Mixed Bag" crossfading into "Lossless 16" (44.1 kHz FLAC, in album
//! order after it). Events go through the app's own calls; the volume is 0, as every bench run.
//!
//! - crossfade on (1 s), technical details shown;
//! - after the overlap's middle the 44.1 kHz track is current, and the device was not reopened:
//!   the output line, Settings > Output's line and the player bar say it is converted from
//!   44.1 kHz ("→ 48"), and the engine reports it;
//! - the album's next track follows gaplessly, still converted (no reopen inside an album);
//! - a pause and a resume: the track plays at its own 44.1 kHz again, and the details say so;
//! - nothing runs while paused is the `idle-paused` bench's business; this one only checks the
//!   words and the rates.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use library::query;
use slint::ComponentHandle;

use crate::bench::{Step, run_steps, snapshot, write_report};
use crate::state::State;
use crate::{AppWindow, Output, Page, Player, playback, views};

type Report = Rc<RefCell<String>>;

fn line(r: &Report, text: String) {
    log::info!("xfade-rates bench: {text}");
    r.borrow_mut().push_str(&text);
    r.borrow_mut().push('\n');
}

thread_local! {
    static FAILS: Cell<u32> = const { Cell::new(0) };
}

fn check(r: &Report, what: &str, ok: bool, detail: String) {
    if !ok {
        FAILS.with(|f| f.set(f.get() + 1));
        crate::bench::count_fail();
    }
    line(r, format!("  [{}] {what}: {detail}", if ok { "ok" } else { "FAIL" }));
}

fn state(st: &State, app: &AppWindow) -> String {
    let s = st.player.status();
    let o = st.player.output_info();
    let p = app.global::<Player>();
    format!(
        "token {:?} at {:.2} s; device {:?} Hz, converted from {:?}; bar {:?}; line {:?}",
        s.token,
        s.position,
        o.as_ref().map(|o| o.rate),
        o.as_ref().and_then(|o| o.converted_from),
        p.get_details_short().to_string(),
        p.get_details_output().to_string()
    )
}

pub fn run() {
    let r: Report = Rc::default();
    let mut steps: Vec<Step> = Vec::new();
    macro_rules! step {
        ($ms:expr, $f:expr) => {{
            let r = r.clone();
            let f: Box<dyn FnOnce(&Rc<State>, &AppWindow, &Report)> = Box::new($f);
            steps.push(($ms, Box::new(move |st: &Rc<State>, app: &AppWindow| f(st, app, &r))));
        }};
    }
    let shots = |steps: &mut Vec<Step>, name: &'static str| {
        steps.push((700, Box::new(move |_st, app| {
            snapshot(app, name);
        })));
    };
    let first_title = Rc::new(RefCell::new(String::new()));

    let t = first_title.clone();
    step!(800, move |st, _app, r| {
        {
            let mut s = st.settings.borrow_mut();
            s.show_details = true;
            s.crossfade.enabled = true;
            s.crossfade.secs = 1.0;
            s.crossfade.album_gapless = true;
        }
        crate::metadata::details::sync(st);
        let albums = query::all_albums(&st.db, query::AlbumSort::Artist, false).unwrap_or_default();
        let tracks = |name: &str| albums.iter().find(|a| a.title == name).map(|a| query::album_tracks(&st.db, a.id).unwrap_or_default()).unwrap_or_default();
        let opus: Vec<_> = tracks("Mixed Bag").into_iter().filter(|t| t.title == "Opus").collect();
        let cd = tracks("Lossless 16");
        check(r, "the fixtures: an Opus track and a 44.1 kHz FLAC album", opus.len() == 1 && cd.len() >= 2, format!("{} Opus, {} FLAC", opus.len(), cd.len()));
        *t.borrow_mut() = cd.first().map(|t| t.title.clone()).unwrap_or_default();
        let mut queue = opus;
        queue.extend(cd);
        playback::play_tracks(st, queue, 0);
        views::go(st, Page::NowPlaying);
        line(r, format!("the next transition: {:?}", playback::next_transition()));
    });
    step!(1200, |st, app, r| {
        let o = st.player.output_info();
        check(r, "the Opus track plays at 48 kHz, as it is", o.as_ref().is_some_and(|o| o.rate == 48_000 && o.converted_from.is_none()), state(st, app));
        check(r, "into another album: a crossfade", matches!(playback::next_transition(), player_core::Transition::Crossfade { .. }), format!("{:?}", playback::next_transition()));
        // 6 s tracks: to a second and a half before the end (the overlap is the last second).
        playback::seek(st, 4.5);
    });
    shots(&mut steps, "xr-01-before");
    let t = first_title.clone();
    step!(2600, move |st, app, r| {
        let current = st.queue.borrow().current().map(|i| i.track.title.clone()).unwrap_or_default();
        let o = st.player.output_info();
        let p = app.global::<Player>();
        let (bar, out, settings) = (p.get_details_short().to_string(), p.get_details_output().to_string(), app.global::<Output>().get_stream().to_string());
        check(r, "the 44.1 kHz track is current after the overlap's middle", current == *t.borrow(), format!("{current:?}"));
        check(r, "the device stayed at 48 kHz, the track converted from 44.1", o.as_ref().is_some_and(|o| o.rate == 48_000 && o.converted_from == Some(44_100)), state(st, app));
        check(r, "the details say so", out.contains("Converted from 44.1 kHz since the crossfade") && !out.contains("not resampled"), out);
        check(r, "Settings > Output's line too", settings.contains("Converted from 44.1 kHz"), settings);
        check(r, "the player bar: FLAC 16/44.1 → 48", bar.contains("44.1") && bar.ends_with("\u{2192} 48"), bar);
        // Played to its end (a seek would play it at its own rate again): the album's next
        // follows gaplessly, still converted.
    });
    shots(&mut steps, "xr-02-converted");
    step!(5500, |st, app, r| {
        let index = st.queue.borrow().current_index().unwrap_or(usize::MAX);
        let o = st.player.output_info();
        check(r, "the album's next track, gapless, still converted (no reopen inside the album)", index == 2 && o.as_ref().is_some_and(|o| o.rate == 48_000 && o.converted_from == Some(44_100)), format!("queue at {index}; {}", state(st, app)));
        playback::set_paused(st, true);
    });
    step!(800, |st, _app, _r| playback::set_paused(st, false));
    step!(1500, |st, app, r| {
        let o = st.player.output_info();
        let out = app.global::<Player>().get_details_output().to_string();
        let bar = app.global::<Player>().get_details_short().to_string();
        check(r, "after a pause: at its own 44.1 kHz", o.as_ref().is_some_and(|o| o.rate == 44_100 && o.converted_from.is_none()), state(st, app));
        check(r, "the details say it plays as it is", !out.contains("Converted") && out.contains("44.1 kHz"), out);
        check(r, "the player bar has no arrow", !bar.contains('\u{2192}'), bar);
    });
    shots(&mut steps, "xr-03-own-rate");
    step!(300, |st, _app, r| {
        st.player.stop();
        let fails = FAILS.with(Cell::get);
        line(r, format!("failed checks: {fails}"));
        write_report("xfade-rates", &r.borrow());
        let _ = slint::quit_event_loop();
    });
    run_steps(steps);
}
