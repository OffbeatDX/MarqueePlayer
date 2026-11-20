//! `MP_BENCH=eq-paste`: Settings > Equalizer's Copy and Paste, the preview pasted and
//! imported settings show before they apply, and the filter kinds of the parametric view
//! (`bench-eq-paste.txt`, `p-*.png` at 1920 x 1080 and 960 x 620). Events go through
//! Slint's own dispatch inside the app; no OS input. The clipboard is the bench's stand-in
//! (`explorer::bench_set_clipboard`): the real one is never touched, except on Linux with
//! `MP_BENCH_REAL_CLIPBOARD=1` inside the private headless session (its own Wayland or X11
//! server, nobody's clipboard), where Copy and Paste also go through the toolkit's.
//!
//! - Copy: the button, Ctrl+C on the page, "Copy as" GraphicEQ and EasyEffects from its
//!   menu (pointer and keyboard), each text read back; the pill says so.
//! - Paste, each format: AutoEq's files for one pair of headphones (FiiO FT1) (ParametricEQ by Ctrl+V, FixedBandEQ
//!   imported as a file, GraphicEQ fitted), Room EQ Wizard's export, EasyEffects (with a
//!   band left out) and PulseEffects presets, this app's own JSON; the preview checked
//!   (format, filters, verdict, notes, curves), applied by Enter or the pointer, cancelled
//!   by Esc or Cancel (the equalizer untouched).
//! - Refused: text that is not an equalizer (the pill), a configuration with IIR
//!   coefficients (the preview with the reason, closed by Esc).
//! - The graphic view of a setting it cannot show: nothing changes, it says so.
//! - The parametric view: a band's kind from its menu by the keyboard (Tab, Enter, Down,
//!   Enter) and by the pointer; only the parameters each kind has.
//! - "Find a setting" finds Copy and Paste; the key actions are listed.
//!
//! Needs a music folder (`MP_MUSIC`); any library will do.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use player_core::eq::formats;
use player_core::eq::{Band, Kind, Preamp, Width};
use serde_json::json;
use slint::platform::{Key, WindowEvent};
use slint::{ComponentHandle, Model};

use crate::bench::{Step, menu_labels, out_dir, press, run_steps, snapshot, tabs, write_report};
use crate::state::State;
use crate::{Actions, AppWindow, Dialog, EqPreview, Equalizer, MenuStore, Nav, settings, views};

type Report = Rc<RefCell<String>>;

fn line(r: &Report, text: String) {
    log::info!("eq-paste bench: {text}");
    r.borrow_mut().push_str(&text);
    r.borrow_mut().push('\n');
}

thread_local! {
    static FAILS: Cell<u32> = const { Cell::new(0) };
    /// The setting before a paste that is cancelled.
    static BEFORE: RefCell<String> = const { RefCell::new(String::new()) };
}

fn check(r: &Report, what: &str, ok: bool, detail: String) {
    if !ok {
        FAILS.with(|f| f.set(f.get() + 1));
        crate::bench::count_fail();
    }
    line(r, format!("  [{}] {what}: {detail}", if ok { "ok" } else { "FAIL" }));
}

/// The AutoEq files the core's tests read (AutoEq's results for the FiiO FT1, MIT licence).
const FT1_PARAMETRIC: &str = include_str!("../../../player-core/tests/fixtures/autoeq/FiiO FT1 ParametricEQ.txt");
const FT1_FIXED: &str = include_str!("../../../player-core/tests/fixtures/autoeq/FiiO FT1 FixedBandEQ.txt");
const FT1_GRAPHIC: &str = include_str!("../../../player-core/tests/fixtures/autoeq/FiiO FT1 GraphicEQ.txt");
/// Room EQ Wizard's "Export filter settings as text", Generic equaliser.
const REW: &str = "Filter Settings file\n\nRoom EQ V5.31\nDated: 16-Nov-2026 10:00:00\n\nNotes:Desk speakers\n\nEqualiser: Generic\nleft 1m.mdat\nTarget level: 75.0dB\nFilter  1: ON  PK       Fc   63.70 Hz  Gain  -5.00 dB  Q  4.000\nFilter  2: ON  LS       Fc   200.0 Hz  Gain   3.00 dB  Q  0.707\nFilter  3: ON  HP       Fc   25.00 Hz\nFilter  4: ON  None\n";

fn easyeffects() -> String {
    let band = |ty: &str, mode: &str, f: f64, g: f64, q: f64| json!({"type": ty, "mode": mode, "slope": "x1", "solo": false, "mute": false, "gain": g, "frequency": f, "q": q, "width": 4.0});
    let ch = json!({
        "band0": band("Lo-shelf", "APO (DR)", 105.0, -2.7, 0.7),
        "band1": band("Bell", "RLC (BT)", 1735.0, -1.9, 3.0),
        "band2": band("Bell", "BWC (BT)", 5303.0, -5.6, 4.5),
        "band3": band("Hi-pass", "APO (DR)", 20.0, 0.0, 0.7)
    });
    json!({"output": {"blocklist": [], "equalizer#0": {"bypass": false, "input-gain": -4.0, "output-gain": 0.0, "mode": "IIR", "num-bands": 4, "split-channels": false, "balance": 0.0, "pitch-left": 0.0, "pitch-right": 0.0, "left": ch.clone(), "right": ch}, "plugins_order": ["equalizer#0"]}}).to_string()
}

const PULSEEFFECTS: &str = r#"{"output":{"equalizer":{"state":"true","mode":"IIR","num-bands":"2","input-gain":"-1","output-gain":"0","split-channels":"false","left":{"band0":{"type":"Bell","mode":"RLC (BT)","slope":"x1","solo":"false","mute":"false","gain":"2.5","frequency":"100","q":"1"},"band1":{"type":"Hi-shelf","mode":"RLC (BT)","slope":"x1","solo":"false","mute":"false","gain":"-2","frequency":"8000","q":"0.4"}},"right":{"band0":{"type":"Bell","mode":"RLC (BT)","slope":"x1","solo":"false","mute":"false","gain":"2.5","frequency":"100","q":"1"},"band1":{"type":"Hi-shelf","mode":"RLC (BT)","slope":"x1","solo":"false","mute":"false","gain":"-2","frequency":"8000","q":"0.4"}}}}}"#;

/// Every kind and width, as this app's own preset file.
fn own_json() -> String {
    formats::marquee_json(
        "Every kind",
        &[
            Band::new(Kind::HighPass, 25.0, 0.0, 0.7),
            Band::with_width(Kind::LowShelf, 110.0, 4.0, 0.9, Width::Slope),
            Band::new(Kind::Notch, 60.0, 0.0, 20.0),
            Band::with_width(Kind::Peak, 1000.0, -3.0, 1.0, Width::Octaves),
            Band::new(Kind::BandPass, 3000.0, 0.0, 0.3),
            Band::new(Kind::AllPass, 500.0, 0.0, 0.7),
            Band::new(Kind::HighShelf, 9000.0, -2.0, 0.7),
            Band::new(Kind::LowPass, 18_000.0, 0.0, 0.707),
        ],
        Preamp::Auto,
    )
}

fn size_window(st: &State, w: i32, h: i32) {
    crate::platform::devwindow::resize_client(st.hwnd.get(), w, h);
}

/// Ctrl and a letter, through Slint's own dispatch.
fn ctrl(app: &AppWindow, ch: &str) {
    let w = app.window();
    w.dispatch_event(WindowEvent::KeyPressed { text: Key::Control.into() });
    w.dispatch_event(WindowEvent::KeyPressed { text: ch.into() });
    w.dispatch_event(WindowEvent::KeyReleased { text: ch.into() });
    w.dispatch_event(WindowEvent::KeyReleased { text: Key::Control.into() });
}

fn clip() -> String {
    crate::explorer::bench_clipboard().unwrap_or_default()
}

fn setting(st: &State) -> String {
    let s = st.settings.borrow();
    let c = crate::eq::current(&s);
    format!("{} {:?} {} {:?}", c.enabled, c.preset, c.preamp_auto, c.bands)
}

fn engine(st: &State) -> String {
    let s = st.settings.borrow();
    let p = crate::eq::params(crate::eq::current(&s));
    format!("enabled {}, {} bands {:?}, preamp {:?}", p.enabled, p.bands().len(), p.bands().iter().map(|b| b.kind).collect::<Vec<_>>(), p.preamp)
}

fn preview(app: &AppWindow) -> String {
    let e = app.global::<EqPreview>();
    let notes: Vec<String> = (0..e.get_notes().row_count()).filter_map(|i| e.get_notes().row_data(i)).map(|n| format!("{}:{}", n.kind, n.text)).collect();
    format!(
        "open {}, refused {}, {:?} | {:?} | {:?} ({}) | {} rows | target {} | {:?} | notes {notes:?} | reason {:?}",
        app.global::<Dialog>().get_eq_preview_open(),
        e.get_refused(),
        e.get_format().to_string(),
        e.get_summary().to_string(),
        e.get_verdict().to_string(),
        e.get_verdict_kind(),
        e.get_rows().row_count(),
        !e.get_curve_target().is_empty(),
        e.get_name_line().to_string(),
        e.get_reason().to_string()
    )
}

fn open(app: &AppWindow) -> bool {
    app.global::<Dialog>().get_eq_preview_open()
}

fn kinds(app: &AppWindow) -> Vec<String> {
    let b = app.global::<Equalizer>().get_bands();
    (0..b.row_count()).filter_map(|i| b.row_data(i)).map(|r| format!("{}{}", r.kind_name, if r.has_gain { "" } else { " (no gain)" })).collect()
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

    // ---- Copy ---------------------------------------------------------------------------
    step!(800, |st, _app, _r| {
        size_window(st, 1920, 1080);
        views::settings_section(st, settings::nav::EQUALIZER);
        crate::eq::choose(st, "Bass boost");
        crate::explorer::bench_set_clipboard(None);
    });
    shots(&mut steps, "p-01-panel");
    step!(300, |_st, app, r| {
        app.global::<Actions>().invoke_eq_copy();
        let text = clip();
        check(r, "Copy: Equalizer APO text, the preset named, the auto preamp's value", text.starts_with("# Marquee Player equalizer preset: Bass boost\n# Marquee Player: preamp auto\nPreamp: -") && text.lines().filter(|l| l.starts_with("Filter ")).count() == 10, text.lines().take(4).collect::<Vec<_>>().join(" | "));
        check(r, "the sliders as the peaks they are", text.contains("Filter 1: ON PK Fc 31.5 Hz Gain 5.4 dB Q 1.41"), text.lines().nth(3).unwrap_or_default().to_owned());
        check(r, "the pill says so", app.global::<Nav>().get_flash() == "Equalizer copied", app.global::<Nav>().get_flash().to_string());
        let back = formats::read(&text, None);
        check(r, "and it reads back as the same setting", back.as_ref().is_ok_and(|p| p.bands.len() == 10 && p.preamp == Some(Preamp::Auto)), format!("{:?}", back.map(|p| p.format)));
    });
    step!(300, |_st, app, r| {
        crate::explorer::bench_set_clipboard(None);
        app.invoke_focus_shortcuts();
        ctrl(app, "c");
        check(r, "Ctrl+C on the page copies too", clip().starts_with("# Marquee Player equalizer preset: Bass boost"), clip().lines().next().unwrap_or_default().to_owned());
    });
    step!(300, |_st, app, r| {
        // "Copy as": the pointer opens the menu, the keyboard chooses the second entry.
        app.global::<Actions>().invoke_eq_copy_menu(1500.0, 260.0);
        let labels = menu_labels(app);
        check(r, "Copy as: its formats", labels.len() == 3 && labels[1].starts_with("GraphicEQ") && labels[2].starts_with("EasyEffects"), format!("{labels:?}"));
    });
    shots(&mut steps, "p-02-copy-as-menu");
    step!(300, |_st, app, r| {
        press(app, Key::DownArrow, &[]);
        press(app, Key::DownArrow, &[]);
        press(app, Key::Return, &[]);
        let text = clip();
        check(r, "Copy as GraphicEQ by the keyboard: AutoEq's 127 points", text.starts_with("GraphicEQ: 20 ") && text.matches(';').count() == 126, text.chars().take(60).collect());
        check(r, "the pill names the format", app.global::<Nav>().get_flash().contains("GraphicEQ"), app.global::<Nav>().get_flash().to_string());
        app.global::<Actions>().invoke_eq_copy_menu(1500.0, 260.0);
        app.global::<Actions>().invoke_menu_activate(2, -1);
        let text = clip();
        let back = formats::read(&text, None);
        check(r, "Copy as EasyEffects by the pointer: a preset EasyEffects reads, read back the same", back.as_ref().is_ok_and(|p| p.format == formats::Format::EasyEffects && p.bands.len() == 10), format!("{:?}", back.map(|p| p.bands.len())));
    });

    // ---- Paste: Equalizer APO / AutoEq, by Ctrl+V; cancelled by Esc ---------------------
    step!(300, |st, app, _r| {
        BEFORE.with(|b| *b.borrow_mut() = setting(st));
        crate::explorer::bench_set_clipboard(Some(FT1_PARAMETRIC));
        app.invoke_focus_shortcuts();
        ctrl(app, "v");
    });
    step!(900, |_st, app, r| {
        let p = preview(app);
        let e = app.global::<EqPreview>();
        check(r, "Ctrl+V: the preview, before anything changes", open(app) && !e.get_refused(), p.clone());
        check(r, "what it is: Equalizer APO text, 10 filters, the preamp", e.get_format() == "Equalizer APO text" && e.get_summary().contains("10 filters") && e.get_summary().contains("4.3"), p.clone());
        check(r, "exact", e.get_verdict_kind() == 0 && e.get_verdict().starts_with("Exact") && e.get_notes().row_count() == 0, p.clone());
        check(r, "the curves: new and now", !e.get_curve_new().is_empty() && !e.get_curve_now().is_empty(), String::new());
    });
    shots(&mut steps, "p-03-preview-apo");
    step!(300, |st, app, r| {
        press(app, Key::Escape, &[]);
        let same = BEFORE.with(|b| *b.borrow() == setting(st));
        check(r, "Esc: closed, the equalizer untouched", !open(app) && same, setting(st));
    });
    // ---- by the keyboard to the Paste button, applied by Enter ------------------------------
    step!(300, |_st, app, _r| {
        app.invoke_focus_shortcuts();
        // The filter box, the section list, the switch, the preset, Copy, Copy as, Paste.
        tabs(app, 7);
        press(app, Key::Return, &[]);
    });
    step!(900, |st, app, r| {
        check(r, "Tab to Paste and Enter: the preview", open(app) && app.global::<EqPreview>().get_rows().row_count() == 10, preview(app));
        press(app, Key::Return, &[]);
        let s = setting(st);
        check(r, "Enter on Apply: applied as a preset, playing", !open(app) && app.global::<Equalizer>().get_preset() == "Pasted equalizer" && engine(st).contains("10 bands"), format!("{}; {}", app.global::<Equalizer>().get_preset(), engine(st)));
        check(r, "with the file's preamp", s.contains("false") && app.global::<Equalizer>().get_preamp().contains("4.3"), app.global::<Equalizer>().get_preamp().to_string());
        check(r, "the pill says so", app.global::<Nav>().get_flash().starts_with("Applied"), app.global::<Nav>().get_flash().to_string());
    });

    // ---- A file: AutoEq's FixedBandEQ, the same preview; applied by the pointer ----------
    step!(300, |st, app, r| {
        let path = out_dir().join("FiiO FT1 FixedBandEQ.txt");
        let _ = std::fs::write(&path, FT1_FIXED);
        let res = crate::eq::import_file(st, &path);
        check(r, "Import from a file: read", res.is_ok(), format!("{res:?}"));
        let _ = app;
    });
    step!(900, |st, app, r| {
        let e = app.global::<EqPreview>();
        check(r, "a file shows the same preview, its format from its content and name", open(app) && e.get_format() == "AutoEq fixed band EQ" && e.get_title().contains("FixedBandEQ"), preview(app));
        app.global::<Actions>().invoke_eq_preview_accept();
        check(r, "applied, named for the headphones", app.global::<Equalizer>().get_preset() == "FiiO FT1" && engine(st).contains("10 bands"), format!("{}; {}", app.global::<Equalizer>().get_preset(), engine(st)));
    });

    // ---- GraphicEQ: fitted, the curve drawn, applied --------------------------------------
    step!(300, |st, _app, _r| {
        crate::explorer::bench_set_clipboard(Some(FT1_GRAPHIC));
        crate::eq::paste(st);
    });
    step!(1200, |_st, app, r| {
        let e = app.global::<EqPreview>();
        check(r, "GraphicEQ: fitted with bands, its own curve drawn too", open(app) && e.get_format() == "GraphicEQ curve" && !e.get_curve_target().is_empty() && e.get_rows().row_count() > 10, preview(app));
        check(r, "the fit's closeness in the verdict and a note", e.get_verdict().starts_with("A close match") && (0..e.get_notes().row_count()).filter_map(|i| e.get_notes().row_data(i)).any(|n| n.text.contains("RMS")), preview(app));
    });
    shots(&mut steps, "p-04-preview-graphiceq");
    step!(300, |st, app, r| {
        app.global::<Actions>().invoke_eq_preview_accept();
        check(r, "applied", engine(st).contains("bands") && app.global::<Equalizer>().get_preset().starts_with("Pasted equalizer"), engine(st));
    });

    // ---- Room EQ Wizard: cancelled by Cancel ------------------------------------------------
    step!(300, |st, _app, _r| {
        BEFORE.with(|b| *b.borrow_mut() = setting(st));
        crate::explorer::bench_set_clipboard(Some(REW));
        crate::eq::paste(st);
    });
    step!(900, |st, app, r| {
        let e = app.global::<EqPreview>();
        check(r, "Room EQ Wizard: its equaliser named, None slots skipped, the corner shelf said", e.get_format() == "Room EQ Wizard filters (Generic)" && e.get_rows().row_count() == 3 && e.get_notes().row_count() == 1, preview(app));
        app.global::<Actions>().invoke_eq_preview_cancel();
        let same = BEFORE.with(|b| *b.borrow() == setting(st));
        check(r, "Cancel: closed, the equalizer untouched", !open(app) && same, String::new());
    });

    // ---- EasyEffects: one band left out, said -----------------------------------------------
    step!(300, |st, _app, _r| {
        crate::explorer::bench_set_clipboard(Some(&easyeffects()));
        crate::eq::paste(st);
    });
    step!(900, |_st, app, r| {
        let e = app.global::<EqPreview>();
        let notes: Vec<String> = (0..e.get_notes().row_count()).filter_map(|i| e.get_notes().row_data(i)).filter(|n| n.kind == 2).map(|n| n.text.to_string()).collect();
        check(r, "EasyEffects: what it is, the band it cannot play left out and said", e.get_format() == "EasyEffects preset" && e.get_rows().row_count() == 3 && e.get_verdict_kind() == 2 && notes.iter().any(|n| n.contains("BWC")), preview(app));
    });
    shots(&mut steps, "p-05-preview-easyeffects");
    step!(300, |st, app, r| {
        app.global::<Actions>().invoke_eq_preview_accept();
        check(r, "applied without it", engine(st).contains("3 bands"), engine(st));
        crate::explorer::bench_set_clipboard(Some(PULSEEFFECTS));
        crate::eq::paste(st);
    });
    step!(900, |_st, app, r| {
        let e = app.global::<EqPreview>();
        check(r, "PulseEffects (every value a string)", e.get_format() == "PulseEffects preset" && e.get_rows().row_count() == 2 && e.get_verdict_kind() == 0, preview(app));
        // Shown again before the last preview had gone: it still has the keyboard, so Esc
        // closes it and does not go back.
        press(app, Key::Escape, &[]);
    });
    step!(300, |st, app, r| {
        check(r, "Esc closed it, still on Settings > Equalizer", !open(app) && crate::eq::shown(st), format!("{:?}", st.history.borrow().current));
    });

    // ---- This app's own file: every kind; the parametric view --------------------------------
    step!(300, |st, _app, _r| {
        crate::explorer::bench_set_clipboard(Some(&own_json()));
        crate::eq::paste(st);
    });
    step!(900, |st, app, r| {
        let e = app.global::<EqPreview>();
        check(r, "this app's own preset file: every kind, exact", e.get_format() == "Marquee Player preset" && e.get_rows().row_count() == 8 && e.get_verdict_kind() == 0, preview(app));
        app.global::<Actions>().invoke_eq_preview_accept();
        check(r, "applied: the bands in full", app.global::<Equalizer>().get_preset() == "Every kind" && !app.global::<Equalizer>().get_graphic(), format!("{:?}", kinds(app)));
        let k = kinds(app);
        check(r, "each kind with only its own parameters", k.len() == 8 && k[0] == "High-pass (no gain)" && k[1] == "Low shelf" && k[2] == "Notch (no gain)", format!("{k:?}"));
        let _ = st;
    });
    shots(&mut steps, "p-06-parametric-kinds");

    // ---- The graphic view of what it cannot show: nothing changes, said ----------------------
    step!(300, |st, app, r| {
        let before = setting(st);
        app.global::<Actions>().invoke_eq_mode(0);
        let note = app.global::<Equalizer>().get_graphic_note().to_string();
        check(r, "Graphic: the sliders shown, the bands kept, why said", app.global::<Equalizer>().get_graphic() && before == setting(st) && note.contains("8 bands") && note.contains("low-pass") && note.contains("Nothing changes"), note);
    });
    shots(&mut steps, "p-07-graphic-note");
    step!(300, |_st, app, _r| app.global::<Actions>().invoke_eq_mode(1));

    // ---- A band's kind: by the keyboard and by the pointer -----------------------------------
    step!(300, |_st, app, _r| {
        app.invoke_focus_shortcuts();
        // The filter box, the list, the switch, the preset, Copy, Copy as, Paste, the view,
        // Reset, the first band's kind.
        tabs(app, 10);
        press(app, Key::Return, &[]);
    });
    step!(400, |st, app, r| {
        let labels = menu_labels(app);
        check(r, "Enter on a band's kind: its menu, every kind", app.global::<MenuStore>().get_open() && labels.len() == 8 && labels[3] == "Low-pass", format!("{labels:?}"));
        // Down to the fourth entry, Low-pass, and Enter.
        for _ in 0..4 {
            press(app, Key::DownArrow, &[]);
        }
        press(app, Key::Return, &[]);
        check(r, "Down to Low-pass and Enter: the band is a low-pass", kinds(app).first().map(String::as_str) == Some("Low-pass (no gain)") && engine(st).contains("LowPass"), format!("{:?}", kinds(app)));
    });
    step!(300, |st, app, r| {
        app.global::<Actions>().invoke_eq_band_kind_menu(1, 600.0, 400.0);
        app.global::<Actions>().invoke_menu_activate(0, -1);
        check(r, "the pointer: the second band a peak, its gain back", kinds(app).get(1).map(String::as_str) == Some("Peak"), format!("{:?}", kinds(app)));
        let q0 = app.global::<Equalizer>().get_bands().row_data(0).map(|b| b.q.to_string()).unwrap_or_default();
        app.global::<Actions>().invoke_eq_band_step(0, "gain".into(), 1);
        app.global::<Actions>().invoke_eq_band_step(0, "q".into(), 1);
        let row = app.global::<Equalizer>().get_bands().row_data(0);
        check(r, "a low-pass has no gain to step; its Q steps", row.as_ref().is_some_and(|b| b.gain == "\u{2014}" && b.q != q0.as_str() && b.width_label == "Q"), format!("{:?}", row.map(|b| (b.gain, b.q))));
        check(r, "the engine has it", engine(st).starts_with("enabled true, 8 bands [LowPass, Peak"), engine(st));
    });
    shots(&mut steps, "p-08-parametric-edited");

    // ---- Refused ------------------------------------------------------------------------------
    step!(300, |st, app, r| {
        BEFORE.with(|b| *b.borrow_mut() = setting(st));
        crate::explorer::bench_set_clipboard(Some("Shopping: milk, eggs\nhttps://example.com/a:b"));
        crate::eq::paste(st);
        let _ = (app, r);
    });
    step!(700, |st, app, r| {
        let flash = app.global::<Nav>().get_flash().to_string();
        check(r, "text that is not an equalizer: said plainly, no preview, nothing changed", !open(app) && flash == "Nothing to paste: no equalizer settings on the clipboard" && BEFORE.with(|b| *b.borrow() == setting(st)), flash);
        crate::explorer::bench_set_clipboard(Some("Preamp: -3 dB\nFilter: ON IIR Order 2 Coefficients 0.0380602 0.0761205 0.0380602 1.2706 -1.84776 0.729402\n"));
        crate::eq::paste(st);
    });
    step!(800, |_st, app, r| {
        let e = app.global::<EqPreview>();
        check(r, "IIR coefficients: the preview says why, with Close only", open(app) && e.get_refused() && e.get_reason().contains("one sample rate"), preview(app));
    });
    shots(&mut steps, "p-09-refused");
    step!(300, |st, app, r| {
        press(app, Key::Escape, &[]);
        check(r, "closed by Esc, nothing changed", !open(app) && BEFORE.with(|b| *b.borrow() == setting(st)), String::new());
    });

    // ---- Find a setting, the keys -------------------------------------------------------------
    step!(300, |_st, _app, r| {
        let hits: Vec<String> = settings::nav::find("paste").into_iter().map(|h| h.label).collect();
        check(r, "Find a setting: Paste, and its key", hits.iter().any(|h| h == "Paste equalizer settings") && hits.iter().any(|h| h.starts_with("Paste equalizer settings, shown")), format!("{hits:?}"));
        let hits: Vec<String> = settings::nav::find("autoeq").into_iter().map(|h| h.label).collect();
        check(r, "and by what it is about", hits.iter().any(|h| h == "Paste equalizer settings"), format!("{hits:?}"));
    });

    // ---- The minimum window size ---------------------------------------------------------------
    step!(300, |st, _app, _r| {
        size_window(st, 960, 620);
        views::settings_section(st, settings::nav::EQUALIZER);
        crate::explorer::bench_set_clipboard(Some(FT1_GRAPHIC));
        crate::eq::paste(st);
    });
    shots(&mut steps, "p-10-small-preview");
    step!(300, |_st, app, _r| {
        // Scrolled to the filters by the keyboard.
        press(app, Key::End, &[]);
    });
    shots(&mut steps, "p-11-small-preview-end");
    step!(300, |st, app, _r| {
        press(app, Key::Escape, &[]);
        crate::eq::choose(st, "FiiO FT1");
    });
    shots(&mut steps, "p-12-small-panel");

    // ---- The real clipboard, only where it is nobody's (Linux, the private session) ------------
    #[cfg(target_os = "linux")]
    step!(300, |_st, _app, r| {
        if std::env::var("MP_BENCH_REAL_CLIPBOARD").as_deref() == Ok("1") {
            let text = "# real clipboard\nPreamp: -1 dB\nFilter 1: ON PK Fc 100 Hz Gain 2 dB Q 1\n";
            let copied = crate::platform::shell::copy_text(text);
            let back = crate::platform::shell::paste_text(formats::MAX_BYTES);
            check(r, "the real clipboard (the toolkit's, Wayland or X11): copied and read back, lines kept", copied && back.as_ref().ok().and_then(|b| b.as_deref()) == Some(text), format!("{copied}, {back:?}"));
        } else {
            line(r, "the real clipboard: not used (MP_BENCH_REAL_CLIPBOARD is not 1)".into());
        }
    });

    step!(300, |st, _app, r| {
        crate::eq::reset(st);
        crate::eq::toggle(st);
        crate::explorer::bench_set_clipboard(None);
        let fails = FAILS.with(Cell::get);
        line(r, format!("failed checks: {fails}"));
        write_report("eq-paste", &r.borrow());
        let _ = slint::quit_event_loop();
    });
    run_steps(steps);
}
