//! theme.toml: the one token file. Colours, radii, font, sizes, motion. Saved edits
//! apply live (the file is watched). A missing file is written out with the defaults,
//! so every token is there to edit; a broken file keeps the last good theme.
//!
//! `accent` is the accent colour that follows the playing track's art.

pub(crate) mod accent;

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;
use slint::{Color, ComponentHandle};

use crate::{AppWindow, Theme};

/// Written when theme.toml does not exist, and the base every user file is merged onto.
const DEFAULT_THEME: &str = r##"# Marquee Player theme. Save to apply; no restart needed.
# Colours are #rrggbb or #rrggbbaa. Sizes are logical pixels, durations milliseconds.

[colors]
bg = "#0c0b10"
surface = "#17151e"
surface_hi = "#221f2b"
border = "#ffffff14"
text = "#f3f0f7"
text_dim = "#a9a3b6"
text_faint = "#928da0"
# The accent pinned when [accent] source = "fixed", and the text on it.
accent = "#ffb347"
accent_text = "#1a1206"
hover = "#ffffff10"
press = "#ffffff1c"
# Tint over the blurred album art behind pages and the player bar.
backdrop_tint = "#0c0b10b0"
bar_tint = "#0c0b10c8"

[backdrop]
# How much of the blurred art shows through (0 to 1).
opacity = 0.55

[accent]
# "art": the accent follows the playing track's cover (a vivid colour from the art,
# clamped to stay legible). "fixed": always [colors] accent.
source = "art"
# Used when nothing is playing or the art has no usable colour (greyscale covers).
fallback = "#b4a6e8"
# How long the accent takes to drift to the next track's colour.
transition_ms = 1200

[glass]
# Live blur of the page behind the sidebar, player bar, queue and navigation pill.
# Strength in pixels (standard deviation); 0 turns it off (opaque panels, and the
# player bar shows the pre-blurred album art instead).
blur = 28
# Tint over the blur; the alpha sets how much of the page shows through.
tint = "#17151ec2"

[shape]
radius_cover = 8
radius_card = 12
radius_control = 8

[type]
font_family = @FONT@
font_small = 13
font_body = 14
font_title = 16
font_heading = 22
font_display = 40

[size]
cover = 184
sidebar_width = 232
queue_width = 340
bar_height = 88
page_margin = 40

[visualizer]
# Settings > Visualizer chooses the style, colours, reflection, motion and where it shows;
# a choice made there wins over this file. These keys still apply: `height` is the band's
# height behind the page (above its reflection), `bar_width` and `bar_gap` the bars' size
# at Medium detail, and the rest are what the Classic look (the default) uses.
# Classic: the bars' strongest alpha (0 to 1); they stay behind the cover and the text.
opacity = 0.4
# Height of the tallest bar.
height = 200
bar_width = 10
bar_gap = 4
# Classic: the mirrored reflection under the bars, between them and the player bar: its
# height as a share of the bars' (0 turns it off), and its alpha where it meets them as a
# share of theirs (0 to 1); it fades out below.
reflection = 0.45
reflection_opacity = 0.55
# Classic: caps that hold a moment above each bar's peak.
peaks = true

[autoscroll]
# Middle-click scrolling (Settings > Window). Around the anchor, this many px on each
# side scroll nothing; it is also how far the pointer must go with the button held for
# the release to end it.
dead_zone = 15
# Px per second with the pointer 100 px past the dead zone; the speed grows with the
# distance to this power (1 is linear; above 1 it is gentle near the anchor and quick far
# from it), up to max_speed.
speed = 600
exponent = 1.8
max_speed = 12000

[tray]
# The tray icon's bars follow the music this many times a second while it plays
# (Settings > Window > Animated tray icon).
fps = 5

[motion]
hover = 120
sidebar = 220
page = 180
backdrop = 600
# The sidebar stays out this long after the pointer leaves it.
sidebar_linger = 250
# With Settings > Auto-hide player bar: the bar stays this long once nothing holds it.
bar_linger = 600
"##;

#[derive(Debug, Deserialize)]
struct ThemeFile {
    colors: Colors,
    backdrop: BackdropT,
    accent: AccentT,
    glass: Glass,
    shape: Shape,
    #[serde(rename = "type")]
    type_: TypeT,
    size: Size,
    visualizer: VisualizerT,
    autoscroll: AutoscrollT,
    tray: TrayT,
    motion: Motion,
}

#[derive(Debug, Deserialize)]
struct AutoscrollT {
    dead_zone: f32,
    speed: f32,
    exponent: f32,
    max_speed: f32,
}

#[derive(Debug, Deserialize)]
struct TrayT {
    fps: u32,
}

#[derive(Debug, Deserialize)]
struct VisualizerT {
    opacity: f32,
    height: f32,
    bar_width: f32,
    bar_gap: f32,
    reflection: f32,
    reflection_opacity: f32,
    peaks: bool,
}

#[derive(Debug, Deserialize)]
struct Colors {
    bg: String,
    surface: String,
    surface_hi: String,
    border: String,
    text: String,
    text_dim: String,
    text_faint: String,
    accent: String,
    accent_text: String,
    hover: String,
    press: String,
    backdrop_tint: String,
    bar_tint: String,
}

#[derive(Debug, Deserialize)]
struct BackdropT {
    opacity: f32,
}

#[derive(Debug, Deserialize)]
struct AccentT {
    source: String,
    fallback: String,
    transition_ms: u64,
}

#[derive(Debug, Deserialize)]
struct Glass {
    blur: f32,
    tint: String,
}

#[derive(Debug, Deserialize)]
struct Shape {
    radius_cover: f32,
    radius_card: f32,
    radius_control: f32,
}

#[derive(Debug, Deserialize)]
struct TypeT {
    font_family: String,
    font_small: f32,
    font_body: f32,
    font_title: f32,
    font_heading: f32,
    font_display: f32,
}

#[derive(Debug, Deserialize)]
struct Size {
    cover: f32,
    sidebar_width: f32,
    queue_width: f32,
    bar_height: f32,
    page_margin: f32,
}

#[derive(Debug, Deserialize)]
struct Motion {
    hover: u64,
    sidebar: u64,
    page: u64,
    backdrop: u64,
    sidebar_linger: u64,
    bar_linger: u64,
}

/// Overlay `user` onto `base`, table by table, so a partial file keeps every other token.
fn merge(base: &mut toml::Table, user: toml::Table) {
    for (k, v) in user {
        match (base.get_mut(&k), v) {
            (Some(toml::Value::Table(b)), toml::Value::Table(u)) => merge(b, u),
            (_, v) => {
                base.insert(k, v);
            }
        }
    }
}

/// Defaults changed since a theme.toml may have been written: (section, key, old value
/// as written, new value). A file still holding the old default is moved to the new one
/// (it was never customised); a customised value is left alone.
const UPGRADES: &[(&str, &str, &str, &str)] = &[
    // Readability: faint text was 3.8:1 on the background, 2:1 or less on glass.
    ("colors", "text_faint", "\"#6f6a7c\"", "\"#928da0\""),
    ("glass", "tint", "\"#17151e99\"", "\"#17151ec2\""),
    ("type", "font_small", "12", "13"),
    // The reflection was behind the player bar's glass (which lets about a quarter
    // through), so its alpha was a share above 1; it now shows above the bar.
    ("visualizer", "reflection_opacity", "1.6", "0.55"),
];

/// Rewrite lines of a theme file that still hold an old default. Returns the new text,
/// or `None` when nothing changed.
fn upgrade_text(text: &str) -> Option<String> {
    let mut section = String::new();
    let mut changed = false;
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        let t = line.trim();
        if t.starts_with('[') && t.ends_with(']') {
            section = t[1..t.len() - 1].trim().to_owned();
        }
        let mut replaced = None;
        if let Some((k, v)) = t.split_once('=') {
            let (k, v) = (k.trim(), v.trim());
            for (s, key, old, new) in UPGRADES {
                if section == *s && k == *key && v == *old {
                    let eol = if line.ends_with("\r\n") { "\r\n" } else if line.ends_with('\n') { "\n" } else { "" };
                    replaced = Some(format!("{key} = {new}{eol}"));
                }
            }
        }
        match replaced {
            Some(r) => {
                out.push_str(&r);
                changed = true;
            }
            None => out.push_str(line),
        }
    }
    changed.then_some(out)
}

/// The theme written when theme.toml does not exist: its font is the platform's
/// (`platform::system::THEME_FONT`: Segoe UI Variable on Windows; empty on Linux, the
/// desktop's own UI font, so a change of the desktop's font follows).
fn default_theme() -> String {
    DEFAULT_THEME.replace("@FONT@", crate::platform::system::THEME_FONT)
}

fn builtin() -> ThemeFile {
    toml::from_str(&default_theme()).expect("built-in theme parses")
}

fn parse(text: &str) -> Result<ThemeFile, String> {
    let mut table: toml::Table = toml::from_str(&default_theme()).expect("built-in theme parses");
    let mut user: toml::Table = toml::from_str(text).map_err(|e| e.to_string())?;
    drop_non_numbers(&mut user, "");
    merge(&mut table, user);
    table.try_into().map_err(|e: toml::de::Error| e.to_string())
}

/// TOML's `nan` and `inf` taken out of the user's table, so the default stays: `clamp` would let
/// them through to sizes and the visualizer's geometry.
fn drop_non_numbers(t: &mut toml::Table, at: &str) {
    t.retain(|k, v| match v {
        toml::Value::Float(f) if !f.is_finite() => {
            log::warn!("theme: {at}{k} = {f} is not a number to use; the default is kept");
            false
        }
        toml::Value::Table(inner) => {
            drop_non_numbers(inner, &format!("{at}{k}."));
            true
        }
        _ => true,
    });
}

fn parse_color(s: &str) -> Option<Color> {
    let h = s.trim().strip_prefix('#')?;
    let v = u32::from_str_radix(h, 16).ok()?;
    match h.len() {
        6 => Some(Color::from_argb_u8(0xff, (v >> 16) as u8, (v >> 8) as u8, v as u8)),
        8 => Some(Color::from_argb_u8(v as u8, (v >> 24) as u8, (v >> 16) as u8, (v >> 8) as u8)),
        _ => None,
    }
}

/// The theme in `path`, old defaults moved to their new values. `write_back` (at start): the
/// file is changed to show them too, durably and only when it parses (a theme being saved,
/// which the watcher may see half-written, is never written over).
fn load(path: &Path, write_back: bool) -> Result<ThemeFile, String> {
    let mut text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    if let Some(up) = upgrade_text(&text) {
        text = up;
        let theme = parse(&text)?;
        // The watcher then reloads the same values, which is harmless.
        if write_back && crate::settings::write_durably(path, text.as_bytes()).is_ok() {
            log::info!("theme.toml: moved unchanged defaults to their new values");
        }
        return Ok(theme);
    }
    parse(&text)
}

fn apply(app: &AppWindow, t: &ThemeFile) {
    let g = app.global::<Theme>();
    let c = |s: &str, f: &dyn Fn(Color)| match parse_color(s) {
        Some(col) => f(col),
        None => log::warn!("theme: bad colour {s:?}"),
    };
    let k = &t.colors;
    c(&k.bg, &|v| g.set_bg(v));
    c(&k.surface, &|v| g.set_surface(v));
    c(&k.surface_hi, &|v| g.set_surface_hi(v));
    c(&k.border, &|v| g.set_border(v));
    c(&k.text, &|v| g.set_text(v));
    c(&k.text_dim, &|v| g.set_text_dim(v));
    c(&k.text_faint, &|v| g.set_text_faint(v));
    c(&k.hover, &|v| g.set_hover(v));
    c(&k.press, &|v| g.set_press(v));
    c(&k.backdrop_tint, &|v| g.set_backdrop_tint(v));
    c(&k.bar_tint, &|v| g.set_bar_tint(v));
    // The accent itself is set by `theme/accent.rs`, which follows the playing art.
    let rgb = |s: &str, default: [u8; 3]| {
        parse_color(s).map_or(default, |c| [c.red(), c.green(), c.blue()])
    };
    if !matches!(t.accent.source.as_str(), "art" | "fixed") {
        log::warn!("theme: [accent] source must be \"art\" or \"fixed\", not {:?}", t.accent.source);
    }
    crate::theme::accent::set_config(crate::theme::accent::Config {
        follow_art: t.accent.source != "fixed",
        fixed: (rgb(&k.accent, [0xff, 0xb3, 0x47]), rgb(&k.accent_text, [0x1a, 0x12, 0x06])),
        fallback: rgb(&t.accent.fallback, [0xb4, 0xa6, 0xe8]),
        transition: Duration::from_millis(t.accent.transition_ms.min(60_000)),
    });
    g.set_backdrop_opacity(t.backdrop.opacity.clamp(0.0, 1.0));
    g.set_glass_blur(t.glass.blur.clamp(0.0, 100.0));
    c(&t.glass.tint, &|v| g.set_glass_tint(v));
    g.set_radius_cover(t.shape.radius_cover);
    g.set_radius_card(t.shape.radius_card);
    g.set_radius_control(t.shape.radius_control);
    // Empty: the desktop's own UI font (the default on Linux).
    let family = t.type_.font_family.trim();
    g.set_font_family(if family.is_empty() { crate::platform::system::ui_font() } else { crate::platform::system::font_here(family) }.into());
    g.set_font_small(t.type_.font_small);
    g.set_font_body(t.type_.font_body);
    g.set_font_title(t.type_.font_title);
    g.set_font_heading(t.type_.font_heading);
    g.set_font_display(t.type_.font_display);
    g.set_cover_size(t.size.cover);
    g.set_sidebar_width(t.size.sidebar_width);
    g.set_queue_width(t.size.queue_width);
    g.set_bar_height(t.size.bar_height);
    g.set_page_margin(t.size.page_margin);
    let v = &t.visualizer;
    g.set_viz_opacity(v.opacity.clamp(0.0, 1.0));
    g.set_viz_height(v.height.clamp(40.0, 600.0));
    g.set_viz_bar_width(v.bar_width.clamp(1.0, 64.0));
    g.set_viz_bar_gap(v.bar_gap.clamp(0.0, 64.0));
    g.set_viz_reflection(v.reflection.clamp(0.0, 1.0));
    g.set_viz_reflection_opacity(v.reflection_opacity.clamp(0.0, 1.0));
    g.set_viz_peaks(v.peaks);
    // Classic follows the file: redraw with it.
    crate::visualizer::theme_changed();
    let a = &t.autoscroll;
    crate::autoscroll::set_config(crate::autoscroll::Config {
        dead_zone: a.dead_zone.clamp(0.0, 200.0),
        speed: a.speed.clamp(1.0, 20_000.0),
        exponent: a.exponent.clamp(0.5, 4.0),
        max_speed: a.max_speed.clamp(10.0, 100_000.0),
    });
    crate::tray::anim::set_fps(t.tray.fps);
    let ms = |v: u64| v as i64;
    g.set_dur_hover(ms(t.motion.hover));
    g.set_dur_sidebar(ms(t.motion.sidebar));
    g.set_dur_page(ms(t.motion.page));
    g.set_dur_backdrop(ms(t.motion.backdrop));
    g.set_sidebar_linger(ms(t.motion.sidebar_linger));
    g.set_bar_linger(ms(t.motion.bar_linger));
}

/// Load (creating the file if missing), apply, and watch for edits.
pub fn install(app: &AppWindow, path: PathBuf) -> Option<library::dirwatch::DirWatch> {
    if !path.exists() {
        let _ = std::fs::write(&path, default_theme());
    }
    match load(&path, true) {
        Ok(t) => apply(app, &t),
        Err(e) => {
            log::error!("theme.toml: {e}; using built-in theme");
            apply(app, &builtin());
        }
    }

    // Windows reports changes per folder, and this folder also holds library.db, its WAL
    // and the log: only theme.toml, by name (an editor that replaces the file on save
    // still counts), goes further than the watch's own thread.
    let weak = app.as_weak();
    let file = path.clone();
    let name = path.file_name()?.to_owned();
    let watch = library::dirwatch::watch_dir(path.parent()?, false, move |seen| {
        let library::dirwatch::Seen::Path(p) = seen else { return };
        if p.file_name() != Some(name.as_os_str()) || !p.is_file() {
            return;
        }
        let file = file.clone();
        let weak = weak.clone();
        let _ = slint::invoke_from_event_loop(move || {
            let Some(app) = weak.upgrade() else { return };
            match load(&file, false) {
                Ok(t) => {
                    apply(&app, &t);
                    log::info!("theme reloaded");
                }
                // Editors often save in two steps; a half-written file is not an error worth
                // reverting for. Keep the last good theme.
                Err(e) => log::warn!("theme.toml not applied: {e}"),
            }
        });
    });
    match watch {
        Ok(w) => Some(w),
        Err(e) => {
            log::warn!("theme.toml is not watched: {e}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Old defaults are written back only at start and only when the file parses, whole or not at
    /// all; a file being saved (the watcher's) is read, never written.
    #[test]
    fn the_upgrade_is_written_back_only_whole_and_at_start() {
        let dir = std::env::temp_dir().join(format!("mp-theme-upgrade-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("theme.toml");
        let half = "[colors]\ntext_faint = \"#6f6a7c\"\naccent = [\n";
        std::fs::write(&path, half).unwrap();
        assert!(load(&path, true).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), half, "a theme that does not parse is left alone");
        let old = "[colors]\ntext_faint = \"#6f6a7c\"\n";
        std::fs::write(&path, old).unwrap();
        assert_eq!(load(&path, false).unwrap().colors.text_faint, "#928da0");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), old, "reloaded while being saved: only read");
        assert_eq!(load(&path, true).unwrap().colors.text_faint, "#928da0");
        assert!(std::fs::read_to_string(&path).unwrap().contains("#928da0"), "at start: written back");
        assert!(!dir.join("theme.toml.tmp").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// TOML has `nan` and `inf`, which `clamp` lets through to the visualizer's and autoscroll's
    /// geometry: such a value is not taken, and the default stays.
    #[test]
    fn a_number_that_is_not_one_keeps_the_default() {
        let t = parse("[visualizer]
reflection = nan
bar_gap = -inf
height = 120.0
[autoscroll]
speed = inf
[size]
cover = nan
").unwrap();
        let b = builtin();
        assert_eq!(t.visualizer.reflection, b.visualizer.reflection);
        assert_eq!(t.visualizer.bar_gap, b.visualizer.bar_gap);
        assert_eq!(t.visualizer.height, 120.0, "a number next to it is kept");
        assert_eq!(t.autoscroll.speed, b.autoscroll.speed);
        assert_eq!(t.size.cover, b.size.cover);
    }

    #[test]
    fn colours_parse() {
        assert_eq!(parse_color("#ff8000"), Some(Color::from_argb_u8(255, 255, 128, 0)));
        assert_eq!(parse_color("#ff800080"), Some(Color::from_argb_u8(128, 255, 128, 0)));
        assert_eq!(parse_color("ff8000"), None);
    }

    #[test]
    fn partial_file_keeps_other_tokens() {
        let t = parse("[colors]\naccent = \"#00ff00\"\n").unwrap();
        assert_eq!(t.colors.accent, "#00ff00");
        assert_eq!(t.colors.bg, "#0c0b10");
        assert_eq!(t.size.cover, 184.0);
        assert!(parse("[colors\n").is_err());
        assert_eq!(builtin().motion.sidebar, 220);
        assert_eq!(builtin().accent.source, "art");
        assert_eq!(builtin().visualizer.bar_width, 10.0);
        assert_eq!(builtin().autoscroll.dead_zone, 15.0);
        assert_eq!(builtin().tray.fps, 5);
        let d = crate::autoscroll::Config::default();
        let b = builtin().autoscroll;
        assert_eq!((b.dead_zone, b.speed, b.exponent, b.max_speed), (d.dead_zone, d.speed, d.exponent, d.max_speed), "the file's defaults are the code's");
        assert_eq!(parse("[visualizer]
opacity = 0.2
").unwrap().visualizer.height, 200.0, "a partial [visualizer] keeps the rest");
        // Old defaults move to the new ones; customised values stay.
        let old = "[colors]\ntext_faint = \"#6f6a7c\"\naccent = \"#123456\"\n\n[type]\nfont_small = 12\n[glass]\ntint = \"#00000080\"\n";
        let up = upgrade_text(old).unwrap();
        assert!(up.contains("text_faint = \"#928da0\""));
        assert!(up.contains("font_small = 13"));
        assert!(up.contains("accent = \"#123456\""));
        assert!(up.contains("tint = \"#00000080\""), "a customised tint is kept");
        assert_eq!(upgrade_text(&up), None);
        let viz = upgrade_text("[visualizer]
reflection_opacity = 1.6
").unwrap();
        assert!(viz.contains("reflection_opacity = 0.55"), "the old default moves");
        assert_eq!(upgrade_text("[visualizer]
reflection_opacity = 2.0
"), None, "a tuned one stays");
        let b = builtin().visualizer;
        let d = crate::visualizer::look::ThemeViz::default();
        assert_eq!(
            (b.opacity, b.height, b.bar_width, b.bar_gap, b.reflection, b.reflection_opacity, b.peaks),
            (d.opacity, d.height, d.bar_width, d.bar_gap, d.reflection, d.reflection_opacity, d.peaks),
            "the file's defaults are the code's"
        );
        // A theme.toml written before [glass] existed still gets the glass defaults.
        assert_eq!(parse("[colors]
bg = \"#000000\"
").unwrap().glass.blur, 28.0);
    }
}
