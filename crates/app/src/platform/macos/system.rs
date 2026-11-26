//! macOS: facts about the system, read where the app needs them. The version from
//! `NSProcessInfo`, the appearance and the double-click action from the user's global
//! defaults, reduced motion from `NSWorkspace`'s accessibility settings, times from the C
//! library's time zone rules (`localtime_r`, which follows System Settings' time zone), the
//! power source from IOKit's power sources and Low Power Mode from `NSProcessInfo`.

use std::sync::OnceLock;
use std::time::Duration;

/// Where the app keeps its files (`paths`), as Apple's guidelines put an app's: settings,
/// the library and the log in `~/Library/Application Support/MarqueePlayer`, the caches
/// (safe to delete) in `~/Library/Caches/MarqueePlayer`.
pub fn app_dirs() -> crate::paths::Dirs {
    use std::path::PathBuf;
    let home = std::env::var_os("HOME").map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| PathBuf::from("/tmp"));
    let support = home.join("Library/Application Support/MarqueePlayer");
    crate::paths::Dirs { config: support.clone(), data: support.clone(), state: support, cache: home.join("Library/Caches/MarqueePlayer") }
}

/// The theme's default font (theme.toml, `[type] font_family`): empty, the system's UI font
/// (`ui_font`).
pub const THEME_FONT: &str = "\"\"";

/// Renderers to try (`main::select_renderer`): Skia, on Metal.
/// Skia's OpenGL surface is not in the macOS build (Apple deprecated OpenGL).
pub const HAS_SKIA_OPENGL: bool = false;

pub const RENDERERS: &[(&str, &str)] = &[("skia", "Skia (Metal)")];

/// The menu bar's status item is always the static glyph: each frame of the moving one went
/// through Slint's AppKit tray as a PNG encoded and decoded again, then a menu bar redraw, five
/// times a second for as long as music played (`tray::anim`). Settings does not offer it.
pub const TRAY_ANIMATES: bool = false;

/// At a scale factor of 2 or more the visualizer's filled shapes (the sparks' soft discs of
/// light among them) and the ring's ripples and snare rings are drawn without
/// anti-aliasing (`visualizer::aa_policy`): Skia on Metal has no MSAA here, so every large or
/// concave anti-aliased path was rasterised on the CPU into a mask and uploaded each frame
/// (Halo behind the page 47% of a core with it, 28% with no path anti-aliased; on a Retina
/// display their stair steps are hard to see). The line and its glow keep it.
pub const VIZ_FILLS_ALIASED_AT_2X: bool = true;

/// At a scale factor of 2 or more (with the fills above) the visualizer's line and its glow
/// are smoothed by being drawn as a few strokes without anti-aliasing, one inside the other
/// (`visualizer::aa_policy`, `draw::feather_passes`): Skia's GPU stroker draws those, where an
/// anti-aliased line, too large for Skia's path atlas, was rasterised on the CPU like the
/// fills (Halo behind the page 3.2 ms of CPU a frame, 2.5 ms so; the pictures differ only at
/// the edges, by a few levels of 255).
pub const VIZ_LINES_IN_PASSES_AT_2X: bool = true;

/// The most the seek bar may lag the position while playing, in logical pixels, before the
/// tick sets it between two turns of the time label (`playback::bar_due`). Any change redraws
/// the whole window here (Slint's Skia renderer on Metal has no partial rendering), so a bar set
/// at every physical pixel drew four or five whole frames a second on a track of some minutes;
/// set with the label, which draws a frame a second anyway, it is that one (measured, a 4 minute
/// track in a zoomed window: 9.9 frames a second, 4.4 at every pixel, 1.0 so). A short track,
/// whose bar moves faster than this in a second, steps by it. A seek, a pause, a drag and a new
/// track show at once, as before.
pub const SEEK_STEP: f32 = 3.0;

/// The theme's family as it can be drawn here: as named (Core Text finds a fallback).
pub fn font_here(family: &str) -> String {
    family.to_owned()
}

/// The system's UI font, San Francisco, by the name the font collection gives it on macOS
/// ("System Font"; its files are hidden from the font list under other names).
pub fn ui_font() -> String {
    "System Font".to_owned()
}

/// A string from the user's global defaults (`defaults read -g <key>`).
fn global_default(key: &str) -> Option<String> {
    objc2_foundation::NSUserDefaults::standardUserDefaults().stringForKey(&objc2_foundation::NSString::from_str(key)).map(|s| s.to_string())
}

/// "macOS 15.1.1 (Apple silicon)", read once.
pub fn os_version() -> &'static str {
    static V: OnceLock<String> = OnceLock::new();
    V.get_or_init(|| {
        let v = objc2_foundation::NSProcessInfo::processInfo().operatingSystemVersion();
        let patch = if v.patchVersion > 0 { format!(".{}", v.patchVersion) } else { String::new() };
        let chip = if cfg!(target_arch = "aarch64") { "Apple silicon" } else { "Intel" };
        format!("macOS {}.{}{patch} ({chip})", v.majorVersion, v.minorVersion)
    })
}

/// System Settings > Accessibility > Display > Reduce motion.
pub fn reduced_motion() -> bool {
    objc2_app_kit::NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion()
}

/// What the Mac runs on now: the battery or not (IOKit says the time left is unlimited on
/// external power), and Low Power Mode (macOS 12 and later; never on 11). Read when asked: a
/// call into IOKit and one property, cheap, made when the visualizer starts and when Settings
/// shows the rate, never per frame. `MP_POWER` stands in for it in bench and test builds
/// (`forced_power`).
pub fn power() -> Option<crate::platform::Power> {
    if let Some(p) = forced_power() {
        return Some(p);
    }
    // SAFETY: a plain call without arguments.
    let battery = unsafe { IOPSGetTimeRemainingEstimate() } != TIME_REMAINING_UNLIMITED;
    Some(crate::platform::Power { battery, low_power: low_power_mode() })
}

/// `MP_POWER=battery`, `mains`, either with `,low-power` (Low Power Mode on), or `low-power`
/// alone (on mains): the power source as a test or a bench run states it, since the Mac it
/// runs on may not be unplugged. Honoured in bench and test builds only (`test_override`).
fn forced_power() -> Option<crate::platform::Power> {
    static FORCED: OnceLock<Option<crate::platform::Power>> = OnceLock::new();
    *FORCED.get_or_init(|| {
        let v = crate::test_override("MP_POWER")?;
        let p = parse_power(&v);
        if p.is_none() {
            log::warn!("MP_POWER={v:?} is not battery, mains or low-power; the system's power source is followed");
        }
        p
    })
}

fn parse_power(v: &str) -> Option<crate::platform::Power> {
    let mut p = crate::platform::Power::default();
    for word in v.split(',').map(str::trim) {
        match word {
            "battery" => p.battery = true,
            "mains" => p.battery = false,
            "low-power" => p.low_power = true,
            _ => return None,
        }
    }
    Some(p)
}

/// Low Power Mode is on. Asked only where `NSProcessInfo` has it (macOS 12 and later): on 11
/// the selector does not exist and the message would raise.
fn low_power_mode() -> bool {
    use objc2::runtime::NSObjectProtocol;
    let info = objc2_foundation::NSProcessInfo::processInfo();
    info.respondsToSelector(objc2::sel!(isLowPowerModeEnabled)) && info.isLowPowerModeEnabled()
}

/// Call `changed` on the main thread when the power source changes between external power and
/// the battery, and when Low Power Mode is turned on or off. Notifications, not polling: IOKit's
/// limited-power notification (sent only when the source changes, not as the battery drains)
/// on the main run loop in its common modes (so it comes during a window drag or an open menu
/// too), and Foundation's power state notification on the main queue. Once, at start; nothing
/// with `MP_POWER` set, which stands for the run.
pub fn watch_power(changed: fn()) {
    static CHANGED: OnceLock<fn()> = OnceLock::new();
    if forced_power().is_some() || CHANGED.set(changed).is_err() {
        return;
    }
    extern "C" fn on_source(_: *mut std::ffi::c_void) {
        if let Some(f) = CHANGED.get() {
            f();
        }
    }
    // SAFETY: the callback is a plain function that lives for the whole run and takes no
    // context. The source returned is ours (a create call): the main run loop retains it once
    // added, so ours is released after; it stays scheduled for the whole run.
    unsafe {
        let source = IOPSCreateLimitedPowerNotification(on_source, std::ptr::null_mut());
        if source.is_null() {
            log::warn!("the power source's notification could not be created: the visualizer's automatic rate keeps what it read at start");
        } else {
            CFRunLoopAddSource(CFRunLoopGetMain(), source, kCFRunLoopCommonModes);
            CFRelease(source);
        }
    }
    use objc2::runtime::NSObjectProtocol;
    let info = objc2_foundation::NSProcessInfo::processInfo();
    if !info.respondsToSelector(objc2::sel!(isLowPowerModeEnabled)) {
        return;
    }
    use objc2_foundation::{NSNotification, NSNotificationCenter, NSOperationQueue, NSString};
    let block = block2::RcBlock::new(|_: std::ptr::NonNull<NSNotification>| {
        if let Some(f) = CHANGED.get() {
            f();
        }
    });
    // By its value rather than Foundation's constant, which macOS 11 does not export (the
    // app would not load there): the two are the same string (a test checks it).
    let name = NSString::from_str(POWER_STATE_CHANGED);
    // SAFETY: Foundation posts it on a global queue: the main queue given here runs the block on
    // the main thread. The observer it returns is kept for the whole run.
    let observer = unsafe { NSNotificationCenter::defaultCenter().addObserverForName_object_queue_usingBlock(Some(&name), None, Some(&NSOperationQueue::mainQueue()), &block) };
    std::mem::forget(observer);
}

/// `NSProcessInfoPowerStateDidChangeNotification`'s value.
const POWER_STATE_CHANGED: &str = "NSProcessInfoPowerStateDidChangeNotification";

/// `kIOPSTimeRemainingUnlimited`: on external power.
const TIME_REMAINING_UNLIMITED: f64 = -2.0;

type CFRunLoopRef = *mut std::ffi::c_void;
type CFRunLoopSourceRef = *mut std::ffi::c_void;
type CFStringRef = *const std::ffi::c_void;

// IOKit's power sources (IOKit/ps/IOPowerSources.h, macOS 10.9 and later): declared here, as
// the bindings in the tree do not cover them and two functions do not warrant a crate.
#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IOPSGetTimeRemainingEstimate() -> f64;
    fn IOPSCreateLimitedPowerNotification(callback: extern "C" fn(*mut std::ffi::c_void), context: *mut std::ffi::c_void) -> CFRunLoopSourceRef;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRunLoopGetMain() -> CFRunLoopRef;
    fn CFRunLoopAddSource(rl: CFRunLoopRef, source: CFRunLoopSourceRef, mode: CFStringRef);
    fn CFRelease(cf: *const std::ffi::c_void);
    static kCFRunLoopCommonModes: CFStringRef;
}

/// The menu bar is light: the system's appearance is not Dark (`AppleInterfaceStyle` is set
/// only in Dark mode, and in Auto while it is dark), for the status item glyph's deeper colours.
pub fn light_panel() -> bool {
    global_default("AppleInterfaceStyle").is_none_or(|s| !s.eq_ignore_ascii_case("dark"))
}

/// The status item's icon size: the menu bar's icons are 16 to 18 points; drawn at twice
/// that for a Retina display, which AppKit scales down.
pub fn tray_icon_size() -> u32 {
    36
}

/// The menu bar is always there to hide the window to.
pub fn tray_host() -> bool {
    true
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

/// macOS counts CPU time precisely (`cpu_time`); no cycle counter per process here.
#[cfg(feature = "bench")]
pub fn cycles() -> Option<u64> {
    None
}

/// GDI and USER objects are Windows'; nothing to count here.
#[cfg(feature = "bench")]
pub fn gui_objects() -> (u32, u32) {
    (0, 0)
}

/// This process's memory (its physical footprint now and the largest it has been, in MB) and
/// open file descriptors (the bench's long runs watch them for growth), as Windows gives
/// private bytes and the peak working set and Linux the resident size and its peak.
///
/// The footprint is what Activity Monitor's "Memory" column and `footprint` show: the pages
/// this process owns (dirty and compressed, not the shared libraries' clean pages), and on
/// Apple silicon the GPU memory of its textures and buffers too, which the resident size
/// leaves out. From `proc_pid_rusage` (`rusage_info_v4`, macOS 10.13 and later); should that
/// fail, nothing now and `getrusage`'s peak resident size (bytes on macOS) for the peak.
#[cfg(feature = "bench")]
pub fn resources() -> (f64, f64, u32) {
    const MB: f64 = 1_048_576.0;
    let fds = std::fs::read_dir("/dev/fd").map(|d| d.count().saturating_sub(1) as u32).unwrap_or(0);
    // SAFETY: a plain struct of integers for the kernel to fill, of the flavour asked for
    // (libc declares the buffer as `*mut rusage_info_t`, a pointer to `void *`; the kernel
    // writes the struct itself there).
    let mut info: libc::rusage_info_v4 = unsafe { std::mem::zeroed() };
    // SAFETY: as above; this process's own pid.
    if unsafe { libc::proc_pid_rusage(libc::getpid(), libc::RUSAGE_INFO_V4, (&raw mut info).cast()) } == 0 {
        // The kernel moves the lifetime peak up only now and then, so the footprint now can be
        // a little above it: the peak is never less than now.
        let now = info.ri_phys_footprint as f64 / MB;
        return (now, now.max(info.ri_lifetime_max_phys_footprint as f64 / MB), fds);
    }
    // SAFETY: as in `cpu_time`.
    let mut u: libc::rusage = unsafe { std::mem::zeroed() };
    // SAFETY: as above.
    let peak = if unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut u) } == 0 { u.ru_maxrss as f64 / MB } else { 0.0 };
    (0.0, peak, fds)
}

/// About's credits for what only the macOS build links (`about::credits`).
pub const CREDITS_SOUND: &[crate::about::Credit] = &[crate::about::Credit {
    name: "objc2-audio-toolbox",
    what: "Sound output through Core Audio",
    licence: "Zlib, Apache-2.0 or MIT",
    package: "objc2-audio-toolbox",
    package_licence: "Zlib OR Apache-2.0 OR MIT",
}];
pub const CREDITS_DESKTOP: &[crate::about::Credit] = &[crate::about::Credit {
    name: "souvlaki",
    what: "Media keys and Now Playing",
    licence: "MIT",
    package: "souvlaki",
    package_licence: "MIT",
}];
pub const CREDITS_SYSTEM: &[crate::about::Credit] = &[
    crate::about::Credit { name: "objc2", what: "AppKit and Foundation bindings", licence: "MIT", package: "objc2", package_licence: "MIT" },
    crate::about::Credit { name: "libc", what: "Unix API bindings", licence: "MIT or Apache-2.0", package: "libc", package_licence: "MIT OR Apache-2.0" },
];
/// What wgpu drives under Skia here.
pub const GPU_API_UNDER_SKIA: &str = "Metal under Skia";
/// The target About's credits test checks the dependencies of.
#[cfg_attr(not(test), allow(dead_code))]
pub const TARGET_TRIPLE: &str = "aarch64-apple-darwin";

/// The words that name the system in the UI.
pub const WORDS: crate::platform::Words = crate::platform::Words {
    tagline: "A local-first, artwork-forward music player for macOS.",
    show_in: "Show in Finder",
    show_file: "Show the file in the Finder",
    show_folder: "Show the album's folder in the Finder",
    open_folder_in: " in the Finder",
    copy_names: "macOS",
    copy_label: "Copy the version, macOS version and renderer, for a bug report",
    default_output: "The system default follows the output chosen in System Settings > Sound, also when it changes.",
    exclusive_title: "Exclusive mode",
    exclusive_hint: "Not available on macOS: playback shares the device with other apps, through Core Audio.",
    exclusive_more: "Core Audio plays at the device's own rate (Audio MIDI Setup sets it) and converts each track to it. Volume, ReplayGain and the equalizer apply in the app; the technical details say the rate the device runs at.",
    profile: "home folder",
    autostart: "Open at login",
    autostart_more: "A LaunchAgent in your Library (LaunchAgents) starts it. System Settings > General > Login Items lists it as a background item; turned off there, it does not start at login.",
};

pub fn words() -> crate::platform::Words {
    WORDS
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

    #[test]
    fn the_system_line_names_macos() {
        assert!(os_version().starts_with("macOS "), "{}", os_version());
    }

    /// The memory now is the physical footprint, read (not the stand-in of nothing), and the
    /// largest it has been is no smaller.
    #[cfg(feature = "bench")]
    #[test]
    fn the_memory_is_the_footprint_now_and_its_peak() {
        let (now, peak, fds) = resources();
        assert!(now > 1.0 && now < 1_000_000.0, "{now} MB");
        assert!(peak >= now, "peak {peak} MB, now {now} MB");
        assert!(fds >= 3, "{fds}");
    }

    #[test]
    fn mp_power_states_the_source_and_low_power_mode() {
        use crate::platform::Power;
        assert_eq!(parse_power("battery"), Some(Power { battery: true, low_power: false }));
        assert_eq!(parse_power("mains"), Some(Power { battery: false, low_power: false }));
        assert_eq!(parse_power("low-power"), Some(Power { battery: false, low_power: true }));
        assert_eq!(parse_power("battery, low-power"), Some(Power { battery: true, low_power: true }));
        assert_eq!(parse_power("mains,low-power"), Some(Power { battery: false, low_power: true }));
        assert_eq!(parse_power("unplugged"), None, "a word it does not know: the system's own is followed");
    }

    /// The power source is read (whatever it is on the Mac that runs the tests), and the name
    /// the app observes Low Power Mode by is Foundation's.
    #[test]
    fn the_power_is_read_and_the_notification_is_foundations() {
        assert!(power().is_some());
        // SAFETY: Foundation's constant, there on every macOS the tests run on (12 and later).
        let theirs = unsafe { objc2_foundation::NSProcessInfoPowerStateDidChangeNotification };
        assert_eq!(theirs.to_string(), POWER_STATE_CHANGED);
    }

    #[test]
    fn the_files_are_in_the_library() {
        let d = app_dirs();
        assert!(d.data.ends_with("Library/Application Support/MarqueePlayer"));
        assert!(d.cache.ends_with("Library/Caches/MarqueePlayer"));
    }
}
