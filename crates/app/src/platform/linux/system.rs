//! Linux: facts about the system and the desktop, read where the app needs them.
//!
//! Desktop settings come from the freedesktop settings portal where it answers
//! (`org.freedesktop.appearance`: the colour scheme, reduced motion), else from the
//! desktop's own configuration (KDE's `kdeglobals`; GNOME's `gsettings`). Times use the C
//! library's time zone rules (`localtime_r`: `TZ`, else `/etc/localtime`).

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use super::dbus;

/// Where the app keeps its files (`paths`), by the XDG base directory specification: each
/// `XDG_*_HOME` when set to an absolute path, else its default under the home folder.
pub fn app_dirs() -> crate::paths::Dirs {
    use std::path::PathBuf;
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/tmp"));
    let xdg = |var: &str, default: &str| {
        std::env::var_os(var).map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| home.join(default)).join("marquee-player")
    };
    crate::paths::Dirs {
        config: xdg("XDG_CONFIG_HOME", ".config"),
        data: xdg("XDG_DATA_HOME", ".local/share"),
        cache: xdg("XDG_CACHE_HOME", ".cache"),
        state: xdg("XDG_STATE_HOME", ".local/state"),
    }
}

/// The theme's default font (theme.toml, `[type] font_family`): empty, the desktop's own
/// UI font (`ui_font`), so a change of the desktop's font follows.
pub const THEME_FONT: &str = "\"\"";

/// Renderers to try (`main::select_renderer`): Skia, whose surfaces Slint tries in order when
/// the window is created, falling back if one does not start: OpenGL first (every GPU and
/// driver of the last fifteen years has it, Mesa's and NVIDIA's; with no GPU, Mesa's llvmpipe
/// draws), then Vulkan through wgpu, then Skia's own software surface. All three draw the live
/// blur. (A renderer named here is only created with the window, so a list of names could not
/// fall back: one Skia with its own fallbacks does.)
/// Skia's OpenGL surface is in every build here (`SLINT_BACKEND=winit-skia-opengl` works).
pub const HAS_SKIA_OPENGL: bool = true;

/// The tray icon's bars move while music plays (`tray::anim`, Settings > Window).
pub const TRAY_ANIMATES: bool = true;

/// The visualizer's paths are all anti-aliased at any scale factor (`visualizer::aa_policy`;
/// only macOS draws its fills without at 2x).
pub const VIZ_FILLS_ALIASED_AT_2X: bool = false;

pub const RENDERERS: &[(&str, &str)] = &[("skia", "Skia (OpenGL, else Vulkan through wgpu, else software)")];

/// The theme's family as it can be drawn here: as named (fontconfig finds a fallback).
pub fn font_here(family: &str) -> String {
    family.to_owned()
}

/// The desktop's UI font family: KDE's `font=` in kdeglobals, GNOME's (and its relatives')
/// `font-name`; empty when none says (the toolkit's default through fontconfig, which is
/// the distribution's sans-serif). CJK and other scripts the font lacks fall back through
/// fontconfig to whatever font has them.
pub fn ui_font() -> String {
    static FONT: OnceLock<String> = OnceLock::new();
    FONT.get_or_init(|| {
        let family = kde_font().or_else(gnome_font).unwrap_or_default();
        log::info!("UI font: {}", if family.is_empty() { "the default (fontconfig's sans-serif)" } else { &family });
        family
    })
    .clone()
}

fn config_home() -> std::path::PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config")))
        .unwrap_or_default()
}

/// A key of a section of KDE's kdeglobals.
fn kdeglobals(section: &str, key: &str) -> Option<String> {
    let text = std::fs::read_to_string(config_home().join("kdeglobals")).ok()?;
    let mut inside = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            inside = line == format!("[{section}]");
        } else if inside
            && let Some((k, v)) = line.split_once('=')
            && k.trim() == key
        {
            return Some(v.trim().to_owned());
        }
    }
    None
}

fn kde_font() -> Option<String> {
    // "Noto Sans,10,-1,5,400,0,0,0,0,0,0,0,0,0,0,1"
    kdeglobals("General", "font").and_then(|f| f.split(',').next().map(str::to_owned)).filter(|f| !f.is_empty())
}

fn desktop() -> String {
    std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default()
}

fn gsettings(schema: &str, key: &str) -> Option<String> {
    let out = std::process::Command::new("gsettings").args(["get", schema, key]).stderr(std::process::Stdio::null()).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().trim_matches('\'').to_owned())
}

fn gnome_font() -> Option<String> {
    let d = desktop();
    let schema = if d.contains("MATE") {
        "org.mate.interface"
    } else if ["GNOME", "Unity", "Budgie", "Cinnamon", "Pantheon", "X-Cinnamon"].iter().any(|n| d.contains(n)) {
        "org.gnome.desktop.interface"
    } else {
        return None;
    };
    // "Cantarell 11", "Ubuntu Sans 11": the family is all but the size.
    let name = gsettings(schema, "font-name")?;
    let family = name.rsplit_once(' ').filter(|(_, size)| size.parse::<f32>().is_ok()).map_or(name.as_str(), |(f, _)| f);
    Some(family.to_owned()).filter(|f| !f.is_empty())
}

/// "Ubuntu 24.04.1 LTS, Linux 6.8.0, KDE on Wayland", read once.
pub fn os_version() -> &'static str {
    static V: OnceLock<String> = OnceLock::new();
    V.get_or_init(|| {
        let os = std::fs::read_to_string("/etc/os-release")
            .ok()
            .and_then(|t| t.lines().find_map(|l| l.strip_prefix("PRETTY_NAME=").map(|v| v.trim_matches('"').to_owned())))
            .unwrap_or_else(|| "Linux".to_owned());
        let kernel = {
            // SAFETY: a plain struct for uname to fill.
            let mut u: libc::utsname = unsafe { std::mem::zeroed() };
            // SAFETY: as above.
            if unsafe { libc::uname(&mut u) } == 0 {
                // SAFETY: uname NUL-terminates its fields.
                let r = unsafe { std::ffi::CStr::from_ptr(u.release.as_ptr()) }.to_string_lossy().into_owned();
                r.split(['-', '+']).next().unwrap_or(&r).to_owned()
            } else {
                String::new()
            }
        };
        let session = if std::env::var_os("WAYLAND_DISPLAY").is_some() {
            "Wayland"
        } else if std::env::var_os("DISPLAY").is_some() {
            "X11"
        } else {
            "no display"
        };
        let d = desktop();
        let desktop = d.split(':').next().filter(|d| !d.is_empty()).unwrap_or("a desktop");
        format!("{os}, Linux {kernel}, {desktop} on {session}")
    })
}

/// The desktop asks for less motion: the settings portal's `reduced-motion`, GNOME's
/// animations turned off, or KDE's animation speed at "instant".
pub fn reduced_motion() -> bool {
    if let Some(v) = dbus::portal_setting::<u32>("org.freedesktop.appearance", "reduced-motion") {
        return v == 1;
    }
    if let Some(v) = dbus::portal_setting::<bool>("org.gnome.desktop.interface", "enable-animations") {
        return !v;
    }
    kdeglobals("KDE", "AnimationDurationFactor").and_then(|v| v.parse::<f32>().ok()).is_some_and(|f| f == 0.0)
}

/// The desktop prefers a light colour scheme (the settings portal: 2), for the tray glyph's
/// deeper colours. Most panels are dark; without the portal, dark.
pub fn light_panel() -> bool {
    dbus::portal_setting::<u32>("org.freedesktop.appearance", "color-scheme") == Some(2)
}

/// The tray icon's size: a StatusNotifierItem host scales what it gets to its panel (KDE:
/// 22 px at 100%, more on a larger panel or at a higher scale), so one a little larger than
/// that, which stays crisp at 150%.
pub fn tray_icon_size() -> u32 {
    32
}

/// Is there a tray to hide the window to: a StatusNotifierWatcher with a host registered
/// (KDE's panel, GNOME with an AppIndicator extension, most other panels)? Asked when the
/// window is closed (cheap: two calls on the session bus), remembered for a few seconds.
pub fn tray_host() -> bool {
    thread_local! {
        static CACHE: std::cell::Cell<Option<(Instant, bool)>> = const { std::cell::Cell::new(None) };
    }
    if let Some((at, v)) = CACHE.with(std::cell::Cell::get)
        && at.elapsed() < Duration::from_secs(5)
    {
        return v;
    }
    let watcher = "org.kde.StatusNotifierWatcher";
    let v = dbus::has_owner(watcher) && dbus::property::<bool>(watcher, "/StatusNotifierWatcher", watcher, "IsStatusNotifierHostRegistered").unwrap_or(false);
    let before = CACHE.with(|c| c.replace(Some((Instant::now(), v)))).map(|(_, b)| b);
    if before != Some(v) {
        log::info!("{}", if v { "tray: a StatusNotifierItem host is there" } else { "no tray on this desktop (no StatusNotifierItem host): closing the window quits" });
    }
    v
}

fn local_tm(unix_secs: i64) -> Option<libc::tm> {
    let t = unix_secs as libc::time_t;
    // SAFETY: a plain struct for localtime_r to fill; `t` outlives the call.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: as above (localtime_r is the thread-safe form).
    (!unsafe { libc::localtime_r(&t, &mut tm) }.is_null()).then_some(tm)
}

/// The local time's offset from UTC now, in seconds (daylight saving included).
pub fn local_offset_secs() -> i64 {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    local_tm(now).map_or(0, |tm| tm.tm_gmtoff)
}

/// The local time's offset from UTC at a Unix time, in seconds (the zone's rules for that
/// instant, daylight saving included). The offset now when the conversion fails.
pub fn offset_at(unix_secs: i64) -> i64 {
    local_tm(unix_secs).map_or_else(local_offset_secs, |tm| tm.tm_gmtoff)
}

/// The local date of a Unix time.
pub fn local_date(unix_secs: i64) -> Option<(i64, u32, u32)> {
    local_tm(unix_secs).map(|tm| (i64::from(tm.tm_year) + 1900, (tm.tm_mon + 1) as u32, tm.tm_mday as u32))
}

/// This process's CPU time (user + system), for the bench.
#[cfg_attr(not(feature = "bench"), allow(dead_code))]
pub fn cpu_time() -> Duration {
    // SAFETY: a plain struct for getrusage to fill.
    let mut u: libc::rusage = unsafe { std::mem::zeroed() };
    // SAFETY: as above.
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut u) } != 0 {
        return Duration::ZERO;
    }
    let tv = |t: libc::timeval| Duration::from_secs(t.tv_sec as u64) + Duration::from_micros(t.tv_usec as u64);
    tv(u.ru_utime) + tv(u.ru_stime)
}

/// Linux counts CPU time precisely (`cpu_time`); there is no cycle counter per process.
#[cfg(feature = "bench")]
pub fn cycles() -> Option<u64> {
    None
}

/// GDI and USER objects are Windows'; nothing to count here.
#[cfg(feature = "bench")]
pub fn gui_objects() -> (u32, u32) {
    (0, 0)
}

/// About's credits for what only the Linux build links (`about::credits`).
pub const CREDITS_SOUND: &[crate::about::Credit] = &[
    crate::about::Credit { name: "pipewire-rs", what: "Sound output through PipeWire", licence: "MIT", package: "pipewire", package_licence: "MIT" },
    crate::about::Credit {
        name: "alsa-rs",
        what: "Sound output through ALSA, where there is no PipeWire",
        licence: "MIT or Apache-2.0",
        package: "alsa",
        package_licence: "Apache-2.0/MIT",
    },
];
pub const CREDITS_DESKTOP: &[crate::about::Credit] = &[
    crate::about::Credit {
        name: "zbus",
        what: "D-Bus: the desktop's media controls (MPRIS), the tray and the file manager",
        licence: "MIT",
        package: "zbus",
        package_licence: "MIT",
    },
    crate::about::Credit { name: "ksni", what: "The tray icon (StatusNotifierItem)", licence: "Unlicense", package: "ksni", package_licence: "Unlicense" },
];
pub const CREDITS_SYSTEM: &[crate::about::Credit] = &[
    crate::about::Credit { name: "ureq", what: "HTTPS for Last.fm and the lyrics lookup", licence: "MIT or Apache-2.0", package: "ureq", package_licence: "MIT OR Apache-2.0" },
    crate::about::Credit { name: "rustls", what: "TLS", licence: "Apache-2.0, ISC or MIT", package: "rustls", package_licence: "Apache-2.0 OR ISC OR MIT" },
    crate::about::Credit { name: "libc", what: "Linux API bindings", licence: "MIT or Apache-2.0", package: "libc", package_licence: "MIT OR Apache-2.0" },
];
/// What wgpu drives under Skia here (where OpenGL cannot start).
pub const GPU_API_UNDER_SKIA: &str = "Vulkan under Skia, where OpenGL cannot start";
/// The target About's credits test checks the dependencies of.
#[cfg_attr(not(test), allow(dead_code))]
pub const TARGET_TRIPLE: &str = "x86_64-unknown-linux-gnu";

/// The words that name the system in the UI.
pub const WORDS: crate::platform::Words = crate::platform::Words {
    tagline: "A local-first, artwork-forward music player for Linux.",
    show_in: "Show in file manager",
    show_file: "Show the file in the file manager",
    show_folder: "Show the album's folder in the file manager",
    open_folder_in: " in the file manager",
    copy_names: "the system",
    copy_label: "Copy the version, system and renderer, for a bug report",
    default_output: "The system default follows the desktop's choice of output, also when it changes.",
    exclusive_title: "Bit-perfect playback",
    exclusive_hint: "Asks PipeWire to run at each track's own sample rate, so it is not resampled. Other apps keep playing.",
    exclusive_more: "PipeWire switches its rate where its configuration and the device allow it (default.clock.allowed-rates); other apps at another rate are then resampled instead. The technical details say the rate PipeWire runs at and whether it resamples. Volume, ReplayGain and the equalizer still apply in the app, and a track a crossfade brings in at another rate is converted to the rate playing until the next pause; for bit-perfect output keep the system volume at 100%.",
    profile: "home folder",
    autostart: "Start with the system",
    autostart_more: "The desktop's Autostart settings start apps on Linux.",
};

pub fn words() -> crate::platform::Words {
    WORDS
}

/// This process's memory (resident and peak resident, in MB) and open file descriptors (the
/// bench's long runs watch them for growth).
#[cfg(feature = "bench")]
pub fn resources() -> (f64, f64, u32) {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let kb = |key: &str| status.lines().find_map(|l| l.strip_prefix(key)).and_then(|v| v.split_whitespace().next()?.parse::<f64>().ok()).unwrap_or(0.0);
    let fds = std::fs::read_dir("/proc/self/fd").map(|d| d.count() as u32).unwrap_or(0);
    (kb("VmRSS:") / 1024.0, kb("VmHWM:") / 1024.0, fds)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_dates_follow_the_time_zone() {
        // 2026-07-01 12:00 UTC is that day in every zone of the world.
        assert_eq!(local_date(1_782_907_200), Some((2026, 7, 1)));
        assert!(local_offset_secs().abs() <= 14 * 3600);
    }

    /// The offset at an instant agrees with the offset now and with the local date, in summer
    /// and in winter (whatever this machine's zone).
    #[test]
    fn the_offset_at_an_instant_follows_the_zone_s_rules() {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
        assert_eq!(offset_at(now), local_offset_secs());
        // 2026-01-15 and 2026-07-15, 12:00 UTC.
        for (t, day) in [(1_768_478_400, (2026, 1, 15)), (1_784_116_800, (2026, 7, 15))] {
            let o = offset_at(t);
            assert!(o.abs() <= 14 * 3600 && o % 900 == 0, "{o}");
            let local = t + o;
            let (y, m, d) = library::smart::civil_from_days(local.div_euclid(86_400));
            assert_eq!(Some((y, m, d)), local_date(t));
            assert_eq!((y, m, d), day, "12:00 UTC is that day everywhere");
        }
    }

    #[test]
    fn the_system_line_names_the_kernel_and_the_session() {
        let v = os_version();
        assert!(v.contains(", Linux "), "{v}");
        assert!(v.ends_with("Wayland") || v.ends_with("X11") || v.ends_with("no display"), "{v}");
    }
}
