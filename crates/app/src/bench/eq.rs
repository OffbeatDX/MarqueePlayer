//! `MP_BENCH=eq`: Settings > Equalizer, the player bar's indicator and Settings > Playback's
//! crossfade (`bench-eq.txt`, `q-*.png` at the window's size and at 960 x 620). Events go
//! through Slint's own dispatch inside the app; no OS input.
//!
//! - the panel off, on and flat, with a preset (the sliders, the curve, the indicator lit);
//! - the keyboard: Tab to the sliders, Right to a band, Up and Page Up to raise it; the
//!   engine's setting checked after each change;
//! - the parametric view, an AutoEq file imported (its preview, then its ten bands and its
//!   preamp), a file of IIR coefficients refused, the preset menu, "Save as preset…" by the
//!   keyboard, export as Equalizer APO text and as this app's own file, each read back;
//! - a track playing (the volume at 0, as every bench run): the technical details naming
//!   the equalizer, the device's own setting switched on and off;
//! - the crossfade settings, and the transitions the app hands the engine for an album in
//!   order (gapless), the next album (crossfade) and repeat one (gapless).
//!
//! Needs a music folder (`MP_MUSIC`) with at least two albums of three tracks or more.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use library::query;
use slint::platform::Key;
use slint::{ComponentHandle, Model};

use crate::bench::{Step, menu_labels, out_dir, press, run_steps, snapshot, tabs, type_text, write_report};
use crate::state::State;
use crate::{Actions, AppWindow, Dialog, Equalizer, MenuStore, Page, Player, playback, settings, views};

type Report = Rc<RefCell<String>>;

fn line(r: &Report, text: String) {
    log::info!("eq bench: {text}");
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

fn gains(app: &AppWindow) -> Vec<f32> {
    let g = app.global::<Equalizer>().get_gains();
    (0..g.row_count()).filter_map(|i| g.row_data(i)).collect()
}

fn panel(app: &AppWindow) -> String {
    let e = app.global::<Equalizer>();
    format!(
        "on {}, preset {:?}, graphic {}, gains {:?}, bands {}, preamp {:?} (auto {}), device {:?} (own {})",
        e.get_on(),
        e.get_preset().to_string(),
        e.get_graphic(),
        gains(app),
        e.get_bands().row_count(),
        e.get_preamp().to_string(),
        e.get_auto_preamp(),
        e.get_device().to_string(),
        e.get_per_device()
    )
}

/// What the app last handed the engine, as the setting it was made from.
fn engine_setting(st: &State) -> String {
    let s = st.settings.borrow();
    let p = crate::eq::params(crate::eq::current(&s));
    format!("enabled {}, {} bands, gains {:?}, preamp {:?}", p.enabled, p.bands().len(), p.bands().iter().map(|b| b.gain_db).collect::<Vec<_>>(), p.preamp)
}

fn size_window(st: &State, w: i32, h: i32) {
    crate::platform::devwindow::resize_client(st.hwnd.get(), w, h);
}

/// An AutoEq correction file (the format AutoEq writes: Preamp, then PK / LSC / HSC filters).
const AUTOEQ: &str = "Preamp: -6.6 dB\nFilter 1: ON LSC Fc 105 Hz Gain 6.4 dB Q 0.70\nFilter 2: ON PK Fc 152 Hz Gain -3.2 dB Q 0.42\nFilter 3: ON PK Fc 1384 Hz Gain 2.4 dB Q 1.57\nFilter 4: ON PK Fc 3049 Hz Gain -3.9 dB Q 2.67\nFilter 5: ON PK Fc 5321 Hz Gain 4.1 dB Q 3.44\nFilter 6: ON HSC Fc 10000 Hz Gain -2.6 dB Q 0.70\nFilter 7: ON PK Fc 61 Hz Gain 0.9 dB Q 1.87\nFilter 8: ON PK Fc 7270 Hz Gain -2.1 dB Q 4.61\nFilter 9: ON PK Fc 222 Hz Gain -0.9 dB Q 2.07\nFilter 10: ON PK Fc 19527 Hz Gain -4.1 dB Q 0.62\n";

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

    // ---- the panel --------------------------------------------------------------------
    step!(800, |st, app, r| {
        let s = app.window().size().to_logical(app.window().scale_factor());
        line(r, format!("window {}x{} logical", s.width, s.height));
        check(r, "off by default", !st.settings.borrow().equalizer.all.enabled && !app.global::<Equalizer>().get_on(), panel(app));
        views::settings_section(st, settings::nav::EQUALIZER);
    });
    shots(&mut steps, "q-01-off");
    step!(300, |st, app, r| {
        crate::eq::toggle(st);
        check(r, "on, flat", app.global::<Equalizer>().get_on() && gains(app).iter().all(|&g| g == 0.0), panel(app));
        check(r, "the engine has it", engine_setting(st).starts_with("enabled true, 10 bands"), engine_setting(st));
    });
    shots(&mut steps, "q-02-on-flat");
    step!(300, |st, app, r| {
        crate::eq::choose(st, "Loudness");
        check(r, "a preset", app.global::<Equalizer>().get_preset() == "Loudness" && gains(app)[0] > 7.0, panel(app));
        check(r, "auto preamp takes the largest boost", app.global::<Equalizer>().get_preamp().contains('\u{2212}'), app.global::<Equalizer>().get_preamp().to_string());
    });
    shots(&mut steps, "q-03-loudness");

    // ---- the keyboard -------------------------------------------------------------------
    step!(300, |_st, app, _r| {
        app.invoke_focus_shortcuts();
        // The filter box, the section list, the switch, the preset, Copy, Copy as, Paste, the
        // view, Reset, the sliders.
        tabs(app, 10);
    });
    step!(300, |st, app, r| {
        press(app, Key::RightArrow, &[]);
        press(app, Key::RightArrow, &[]);
        press(app, Key::RightArrow, &[]);
        press(app, Key::UpArrow, &[]);
        press(app, Key::PageUp, &[]);
        let g = gains(app);
        check(r, "Right to the 250 Hz band, Up to the next half dB and Page Up 3 dB", (g[3] - 5.5).abs() < 0.01, format!("{g:?}"));
        check(r, "edited: no longer the preset", app.global::<Equalizer>().get_preset() == "Custom", panel(app));
        check(r, "the engine follows", engine_setting(st).contains("5.5"), engine_setting(st));
    });
    shots(&mut steps, "q-04-keyboard");
    step!(300, |st, app, r| {
        press(app, Key::Home, &[]);
        let low = gains(app)[3];
        press(app, Key::Delete, &[]);
        check(r, "Home to -12, Delete to 0", low == -12.0 && gains(app)[3] == 0.0, format!("{low} then {:?}", gains(app)));
        // The pointer's path: the callback a slider drag calls.
        app.global::<Actions>().invoke_eq_gain(8, 4.26);
        check(r, "a drag snaps to 0.5 dB", gains(app)[8] == 4.5, format!("{:?}", gains(app)));
        let _ = st;
    });

    // ---- parametric, AutoEq, presets ----------------------------------------------------
    step!(300, |st, app, r| {
        crate::eq::set_mode(st, true);
        check(r, "parametric: the ten bands in full", !app.global::<Equalizer>().get_graphic() && app.global::<Equalizer>().get_bands().row_count() == 10, panel(app));
    });
    shots(&mut steps, "q-05-parametric");
    step!(300, |st, app, r| {
        let path = out_dir().join("HD 999 ParametricEQ.txt");
        let _ = std::fs::write(&path, AUTOEQ);
        let res = crate::eq::import_file(st, &path);
        check(r, "an AutoEq file is read", res.is_ok(), format!("{res:?}"));
        let _ = app;
    });
    step!(900, |st, app, r| {
        let p = app.global::<crate::EqPreview>();
        check(r, "shown before it applies", app.global::<Dialog>().get_eq_preview_open() && p.get_format() == "AutoEq parametric EQ" && p.get_rows().row_count() == 10, format!("{:?} {}", p.get_format().to_string(), p.get_rows().row_count()));
        app.global::<Actions>().invoke_eq_preview_accept();
        check(r, "applied as a preset named for the headphones", app.global::<Equalizer>().get_preset() == "HD 999", panel(app));
        check(r, "its ten bands and its preamp", app.global::<Equalizer>().get_bands().row_count() == 10 && app.global::<Equalizer>().get_preamp().contains("6.6"), panel(app));
        let bad = out_dir().join("iir.txt");
        let _ = std::fs::write(&bad, "Filter 1: ON IIR Order 2 Coefficients 0.0380602 0.0761205 0.0380602 1.2706 -1.84776 0.729402\n");
        let _ = crate::eq::import_file(st, &bad);
    });
    step!(900, |_st, app, r| {
        let p = app.global::<crate::EqPreview>();
        check(r, "a file it cannot play as meant is refused, saying why", p.get_refused() && p.get_reason().contains("coefficients"), p.get_reason().to_string());
        app.global::<Actions>().invoke_eq_preview_cancel();
    });
    shots(&mut steps, "q-06-autoeq");
    step!(300, |st, app, r| {
        crate::eq::open_menu(st, 400.0, 300.0);
        let labels = menu_labels(app);
        check(r, "the preset menu: built-in, the user's, then actions", labels.first().map(String::as_str) == Some("Flat") && labels.iter().any(|l| l == "HD 999") && labels.iter().any(|l| l.starts_with("Import")), format!("{labels:?}"));
    });
    shots(&mut steps, "q-07-menu");
    step!(300, |st, app, r| {
        app.global::<MenuStore>().set_open(false);
        crate::eq::save_prompt(st);
        check(r, "Save as preset asks for a name", app.global::<Dialog>().get_prompt_open() && app.global::<Dialog>().get_prompt_placeholder() == "Preset name", app.global::<Dialog>().get_prompt_text().to_string());
    });
    shots(&mut steps, "q-08-save-prompt");
    step!(300, |_st, app, _r| {
        press(app, Key::End, &[Key::Shift]);
        type_text(app, "My cans");
        press(app, Key::Return, &[]);
    });
    step!(400, |st, app, r| {
        let names: Vec<String> = st.settings.borrow().equalizer.presets.iter().map(|p| p.name.clone()).collect();
        check(r, "saved by the keyboard", names.iter().any(|n| n == "My cans") && app.global::<Equalizer>().get_preset() == "My cans" && !app.global::<Dialog>().get_prompt_open(), format!("{names:?}; {}", panel(app)));
        let path = out_dir().join("exported.txt");
        crate::eq::export_file(st, &path, crate::eq::ExportAs::Apo);
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let back = player_core::eq::formats::read(&text, None);
        let want = crate::eq::params(crate::eq::current(&st.settings.borrow()));
        check(r, "exported as Equalizer APO text, read back the same", back.as_ref().is_ok_and(|p| p.bands == want.bands() && p.name.as_deref() == Some("My cans")), text.lines().take(3).collect::<Vec<_>>().join(" | "));
        let path = out_dir().join("exported.json");
        crate::eq::export_file(st, &path, crate::eq::ExportAs::Marquee);
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let back = player_core::eq::formats::read(&text, None);
        check(r, "exported as this app's own preset, read back the same", back.as_ref().is_ok_and(|p| p.bands == want.bands() && p.format == player_core::eq::formats::Format::Marquee(2)), text.lines().take(3).collect::<Vec<_>>().join(" | "));
    });

    // ---- playing: the details, a device's own setting -----------------------------------
    step!(300, |st, _app, _r| {
        st.settings.borrow_mut().show_details = true;
        crate::metadata::details::sync(st);
        let albums = query::all_albums(&st.db, query::AlbumSort::Artist, false).unwrap_or_default();
        if let Some(a) = albums.first() {
            playback::play_tracks(st, query::album_tracks(&st.db, a.id).unwrap_or_default(), 0);
        }
    });
    step!(2500, |st, app, r| {
        let out = app.global::<Player>().get_details_output().to_string();
        check(r, "the technical details name the equalizer", out.contains("Equalizer: My cans"), out);
        check(r, "the device is known", app.global::<Equalizer>().get_device() != "", panel(app));
        views::settings_section(st, settings::nav::EQUALIZER);
    });
    shots(&mut steps, "q-09-playing");
    step!(300, |st, app, r| {
        crate::eq::toggle_per_device(st);
        check(r, "a setting of the device's own", app.global::<Equalizer>().get_per_device(), panel(app));
        crate::eq::choose(st, "Treble reducer");
        let shared = st.settings.borrow().equalizer.all.preset.clone();
        check(r, "changing it leaves the shared one", shared.as_deref() == Some("My cans"), format!("shared {shared:?}; {}", panel(app)));
    });
    shots(&mut steps, "q-10-per-device");
    step!(300, |st, app, r| {
        crate::eq::toggle_per_device(st);
        check(r, "off again: the shared setting plays", app.global::<Equalizer>().get_preset() == "My cans" && !app.global::<Equalizer>().get_per_device(), panel(app));
        views::go(st, Page::NowPlaying);
    });
    shots(&mut steps, "q-11-bar-on");
    step!(300, |st, app, r| {
        crate::eq::toggle(st);
        check(r, "the indicator follows", !app.global::<Equalizer>().get_on(), panel(app));
        let out = app.global::<Player>().get_details_output().to_string();
        check(r, "off: the details say nothing of it", !out.contains("Equalizer"), out);
    });
    shots(&mut steps, "q-12-bar-off");
    step!(300, |st, app, r| {
        app.global::<Actions>().invoke_open_equalizer();
        check(r, "the player bar's button opens Settings > Equalizer", st.history.borrow().current == crate::nav::Route::Settings(settings::nav::EQUALIZER), format!("{:?}", st.history.borrow().current));
        crate::eq::toggle(st);
    });

    // ---- crossfade ------------------------------------------------------------------------
    step!(300, |st, _app, _r| views::settings_section(st, settings::nav::PLAYBACK));
    shots(&mut steps, "q-13-crossfade-off");
    step!(300, |st, app, r| {
        app.global::<Actions>().invoke_crossfade_toggle();
        app.global::<Actions>().invoke_crossfade_step(1);
        let c = st.settings.borrow().crossfade;
        check(r, "crossfade on, 7 s", c.enabled && c.secs == 7.0 && app.global::<Player>().get_crossfade_secs() == "7 s", format!("{c:?}"));
        // The album playing, in order: gapless. Then another album after its last track.
        let albums = query::all_albums(&st.db, query::AlbumSort::Artist, false).unwrap_or_default();
        if albums.len() >= 2 {
            let mut tracks = query::album_tracks(&st.db, albums[0].id).unwrap_or_default();
            let first_len = tracks.len();
            tracks.extend(query::album_tracks(&st.db, albums[1].id).unwrap_or_default());
            playback::play_tracks(st, tracks, 0);
            check(r, "an album in order: gapless", playback::next_transition() == player_core::Transition::Gapless, format!("{:?}", playback::next_transition()));
            st.queue.borrow_mut().jump(first_len - 1);
            playback::play_current(st);
            check(r, "its last track into the next album: crossfade", matches!(playback::next_transition(), player_core::Transition::Crossfade { secs } if secs == 7.0), format!("{:?}", playback::next_transition()));
            playback::cycle_repeat(st);
            playback::cycle_repeat(st);
            check(r, "repeat one: gapless", playback::next_transition() == player_core::Transition::Gapless, format!("{:?}", playback::next_transition()));
            playback::cycle_repeat(st);
        }
    });
    shots(&mut steps, "q-14-crossfade-on");

    // ---- the minimum window size -----------------------------------------------------------
    step!(300, |st, _app, _r| {
        size_window(st, 960, 620);
        views::settings_section(st, settings::nav::EQUALIZER);
        crate::eq::choose(st, "Bass boost");
    });
    shots(&mut steps, "q-15-small-graphic");
    step!(300, |st, _app, _r| crate::eq::choose(st, "HD 999"));
    shots(&mut steps, "q-16-small-parametric");
    step!(300, |st, _app, _r| views::go(st, Page::NowPlaying));
    shots(&mut steps, "q-17-small-bar");
    step!(300, |st, _app, r| {
        st.player.stop();
        crate::eq::reset(st);
        crate::eq::toggle(st);
        let fails = FAILS.with(Cell::get);
        line(r, format!("failed checks: {fails}"));
        write_report("eq", &r.borrow());
        let _ = slint::quit_event_loop();
    });
    run_steps(steps);
}
