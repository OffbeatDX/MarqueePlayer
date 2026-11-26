//! Windows: facts about the system, read where the app needs them (About, the visualizer,
//! the tray, smart playlists, the details).

use std::sync::OnceLock;
use std::time::Duration;

/// Where the app keeps its files (`paths`): the roaming application data folder for
/// settings, the library and the log, the local one for the caches.
pub fn app_dirs() -> crate::paths::Dirs {
    use std::path::PathBuf;
    let env_dir = |var: &str| std::env::var_os(var).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    let roaming = env_dir("APPDATA").join("MarqueePlayer");
    crate::paths::Dirs { config: roaming.clone(), data: roaming.clone(), state: roaming, cache: env_dir("LOCALAPPDATA").join("MarqueePlayer") }
}

/// Renderers to try, in order (`main::select_renderer`): Skia through wgpu, on Direct3D 12
/// (see there for why not OpenGL); then the software renderer.
/// Skia's OpenGL surface only in builds with the `bench-opengl` feature (`select_renderer`).
pub const HAS_SKIA_OPENGL: bool = false;

/// The tray icon's bars move while music plays (`tray::anim`, Settings > Window).
pub const TRAY_ANIMATES: bool = true;

/// The visualizer's paths are all anti-aliased at any scale factor (`visualizer::aa_policy`;
/// only macOS draws its fills without at 2x).
pub const VIZ_FILLS_ALIASED_AT_2X: bool = false;

/// The seek bar is set at every physical pixel it moves (`playback::bar_due`: no step; only
/// macOS, where any change redraws the whole window, sets it with the time label).
pub const SEEK_STEP: f32 = 0.0;

pub const RENDERERS: &[(&str, &str)] = &[("skia-wgpu", "Skia (wgpu)")];

/// The theme's default font (theme.toml, `[type] font_family`).
pub const THEME_FONT: &str = "\"Segoe UI Variable Text\"";

/// The font an empty `font_family` stands for: Windows 11's Segoe UI Variable, or on
/// Windows 10, which does not have it, Segoe UI.
pub fn ui_font() -> String {
    if segoe_variable() { "Segoe UI Variable Text" } else { "Segoe UI" }.to_owned()
}

/// The theme's family as it can be drawn here: the Windows 11 default named in a theme
/// (`THEME_FONT`, written there on first start) becomes Segoe UI on Windows 10.
pub fn font_here(family: &str) -> String {
    if family.to_lowercase().starts_with("segoe ui variable") && !segoe_variable() { "Segoe UI".to_owned() } else { family.to_owned() }
}

/// Is Segoe UI Variable installed (Windows 11 has it; Windows 10 does not)?
fn segoe_variable() -> bool {
    static HAS: OnceLock<bool> = OnceLock::new();
    *HAS.get_or_init(|| registry_string(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Fonts", "Segoe UI Variable (TrueType)").is_some())
}

/// "Windows 11 24H2 (build 26100.2605)", read once.
pub fn os_version() -> &'static str {
    static V: OnceLock<String> = OnceLock::new();
    V.get_or_init(read_windows_version)
}

/// The Windows release by its build number: Windows 11 reports itself as 10.0, from
/// build 22000 on.
pub fn windows_name(major: u32, minor: u32, build: u32, display: &str, ubr: Option<u32>) -> String {
    let name = match (major, minor) {
        (10, 0) if build >= 22_000 => "Windows 11".to_owned(),
        (10, 0) => "Windows 10".to_owned(),
        _ => format!("Windows {major}.{minor}"),
    };
    let display = if display.is_empty() { String::new() } else { format!(" {display}") };
    let build = match ubr {
        Some(u) => format!("{build}.{u}"),
        None => build.to_string(),
    };
    format!("{name}{display} (build {build})")
}

fn read_windows_version() -> String {
    #[repr(C)]
    struct OsVersionInfoW {
        size: u32,
        major: u32,
        minor: u32,
        build: u32,
        platform: u32,
        service_pack: [u16; 128],
    }
    #[link(name = "ntdll")]
    unsafe extern "system" {
        // GetVersionEx answers what the manifest says the app supports; this the truth.
        fn RtlGetVersion(info: *mut OsVersionInfoW) -> i32;
    }
    let mut v = OsVersionInfoW { size: std::mem::size_of::<OsVersionInfoW>() as u32, major: 0, minor: 0, build: 0, platform: 0, service_pack: [0; 128] };
    // SAFETY: RtlGetVersion fills the structure it is given, whose size field is set.
    if unsafe { RtlGetVersion(&mut v) } != 0 {
        return "Windows (version unknown)".into();
    }
    const KEY: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";
    windows_name(v.major, v.minor, v.build, &registry_string(KEY, "DisplayVersion").unwrap_or_default(), registry_dword(KEY, "UBR"))
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

fn registry_string(key: &str, name: &str) -> Option<String> {
    use windows_sys::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW};
    let (key, name) = (wide(key), wide(name));
    let mut buf = [0u16; 128];
    let mut len = std::mem::size_of_val(&buf) as u32;
    // SAFETY: a string read into a local buffer whose size in bytes is passed; the names
    // are NUL-terminated.
    let r = unsafe { RegGetValueW(HKEY_LOCAL_MACHINE, key.as_ptr(), name.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), buf.as_mut_ptr().cast(), &mut len) };
    if r != 0 {
        return None;
    }
    let chars = (len as usize / 2).min(buf.len());
    let s = String::from_utf16_lossy(&buf[..chars]);
    Some(s.trim_end_matches('\0').trim().to_owned()).filter(|s| !s.is_empty())
}

fn registry_dword(key: &str, name: &str) -> Option<u32> {
    use windows_sys::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RegGetValueW};
    let (key, name) = (wide(key), wide(name));
    let mut value: u32 = 0;
    let mut len = std::mem::size_of::<u32>() as u32;
    // SAFETY: a DWORD read into a local of the right size; the names are NUL-terminated.
    let r = unsafe { RegGetValueW(HKEY_LOCAL_MACHINE, key.as_ptr(), name.as_ptr(), RRF_RT_REG_DWORD, std::ptr::null_mut(), (&raw mut value).cast(), &mut len) };
    (r == 0).then_some(value)
}

/// Windows' "Show animations in Windows" (Settings > Accessibility > Visual effects), off.
pub fn reduced_motion() -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{SPI_GETCLIENTAREAANIMATION, SystemParametersInfoW};
    let mut on: i32 = 1;
    // SAFETY: a BOOL out-parameter, as the call documents.
    let ok = unsafe { SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, (&raw mut on).cast(), 0) };
    ok != 0 && on == 0
}

/// The power source is not followed on Windows: the visualizer draws at the rate stored, as
/// chosen (60 unless 30 was), and Settings offers no automatic choice (`platform::Power`).
pub fn power() -> Option<crate::platform::Power> {
    None
}

/// Nothing to watch here (`power`).
pub fn watch_power(_changed: fn()) {}

/// Does the taskbar use the light theme (Windows' "default app mode" for the system,
/// `SystemUsesLightTheme`)?
pub fn light_panel() -> bool {
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
    let key: Vec<u16> = r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize".encode_utf16().chain([0]).collect();
    let name: Vec<u16> = "SystemUsesLightTheme".encode_utf16().chain([0]).collect();
    let mut value: u32 = 0;
    let mut len = std::mem::size_of::<u32>() as u32;
    // SAFETY: a DWORD read into a local of the right size; strings are NUL-terminated.
    let r = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr(), RRF_RT_REG_DWORD, std::ptr::null_mut(), (&raw mut value).cast(), &mut len)
    };
    r == 0 && value == 1
}

/// The notification area's icon size here (the small icon at the system DPI).
pub fn tray_icon_size() -> u32 {
    use windows_sys::Win32::UI::HiDpi::{GetDpiForSystem, GetSystemMetricsForDpi};
    use windows_sys::Win32::UI::WindowsAndMessaging::SM_CXSMICON;
    // SAFETY: plain queries, no pointers.
    let px = unsafe { GetSystemMetricsForDpi(SM_CXSMICON, GetDpiForSystem()) };
    if (8..=256).contains(&px) {
        return px as u32;
    }
    16
}

/// The notification area is always there on Windows (the shell may still refuse an icon:
/// `Tray::new` then fails).
pub fn tray_host() -> bool {
    true
}

/// The local time's offset from UTC now, in seconds (daylight saving included).
pub fn local_offset_secs() -> i64 {
    use windows_sys::Win32::System::Time::{GetTimeZoneInformation, TIME_ZONE_INFORMATION};
    // SAFETY: a plain out-parameter struct, zeroed.
    let mut tz: TIME_ZONE_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: `tz` is valid for writes.
    let id = unsafe { GetTimeZoneInformation(&mut tz) };
    let bias = match id {
        1 => tz.Bias + tz.StandardBias,
        2 => tz.Bias + tz.DaylightBias,
        0 => tz.Bias,
        _ => 0,
    };
    -i64::from(bias) * 60
}

/// The local time's offset from UTC at a Unix time, in seconds (Windows' time zone rules for
/// that instant: daylight saving included, whether or not it is in force now). The offset now
/// (`local_offset_secs`) when the conversion fails.
pub fn offset_at(unix_secs: i64) -> i64 {
    use windows_sys::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows_sys::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
    let t = unix_secs.max(0);
    let ticks = (t + 11_644_473_600) as u64 * 10_000_000;
    let ft = FILETIME { dwLowDateTime: ticks as u32, dwHighDateTime: (ticks >> 32) as u32 };
    let mut utc: SYSTEMTIME = unsafe { std::mem::zeroed() };
    let mut local: SYSTEMTIME = unsafe { std::mem::zeroed() };
    // SAFETY: plain conversions between structures we own; null means the current zone.
    let ok = unsafe { FileTimeToSystemTime(&ft, &mut utc) != 0 && SystemTimeToTzSpecificLocalTime(std::ptr::null(), &utc, &mut local) != 0 };
    if !ok {
        return local_offset_secs();
    }
    let days = library::smart::days_from_civil(i64::from(local.wYear), u32::from(local.wMonth), u32::from(local.wDay));
    days * 86_400 + i64::from(local.wHour) * 3600 + i64::from(local.wMinute) * 60 + i64::from(local.wSecond) - t
}

/// The local date of a Unix time (Windows' time zone rules, daylight saving included).
pub fn local_date(unix_secs: i64) -> Option<(i64, u32, u32)> {
    use windows_sys::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows_sys::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
    let ticks = (unix_secs + 11_644_473_600).max(0) as u64 * 10_000_000;
    let ft = FILETIME { dwLowDateTime: ticks as u32, dwHighDateTime: (ticks >> 32) as u32 };
    let mut utc: SYSTEMTIME = unsafe { std::mem::zeroed() };
    let mut local: SYSTEMTIME = unsafe { std::mem::zeroed() };
    // SAFETY: plain conversions between structures we own; null means the current zone.
    let ok = unsafe { FileTimeToSystemTime(&ft, &mut utc) != 0 && SystemTimeToTzSpecificLocalTime(std::ptr::null(), &utc, &mut local) != 0 };
    ok.then(|| (i64::from(local.wYear), u32::from(local.wMonth), u32::from(local.wDay)))
}

/// This process's CPU time (user + kernel), for the bench.
#[cfg_attr(not(feature = "bench"), allow(dead_code))]
pub fn cpu_time() -> Duration {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
    let z = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
    let (mut c, mut e, mut k, mut u) = (z, z, z, z);
    // SAFETY: plain out-parameters on the current process's pseudo handle.
    if unsafe { GetProcessTimes(GetCurrentProcess(), &mut c, &mut e, &mut k, &mut u) } != 0 {
        let t = |f: FILETIME| (u64::from(f.dwHighDateTime) << 32) | u64::from(f.dwLowDateTime);
        return Duration::from_nanos((t(k) + t(u)) * 100);
    }
    Duration::ZERO
}

/// This process's CPU cycles (all threads), precise where `GetProcessTimes` counts in
/// 15.6 ms ticks (the bench).
#[cfg(feature = "bench")]
pub fn cycles() -> Option<u64> {
    use windows_sys::Win32::System::{Threading::GetCurrentProcess, WindowsProgramming::QueryProcessCycleTime};
    let mut c = 0u64;
    // SAFETY: an out-parameter on this process's pseudo handle.
    (unsafe { QueryProcessCycleTime(GetCurrentProcess(), &mut c) } != 0).then_some(c)
}

/// GDI and USER objects this process holds (the bench checks the tray animation leaks none).
#[cfg(feature = "bench")]
pub fn gui_objects() -> (u32, u32) {
    use windows_sys::Win32::System::Threading::{GR_GDIOBJECTS, GR_USEROBJECTS, GetCurrentProcess, GetGuiResources};
    // SAFETY: plain queries on this process's pseudo handle.
    unsafe { (GetGuiResources(GetCurrentProcess(), GR_GDIOBJECTS), GetGuiResources(GetCurrentProcess(), GR_USEROBJECTS)) }
}

/// About's credits for what only the Windows build links (`about::credits`).
pub const CREDITS_SOUND: &[crate::about::Credit] =
    &[crate::about::Credit { name: "wasapi", what: "Sound output through Windows", licence: "MIT", package: "wasapi", package_licence: "MIT" }];
pub const CREDITS_DESKTOP: &[crate::about::Credit] = &[crate::about::Credit {
    name: "souvlaki",
    what: "Media keys and the Windows media overlay",
    licence: "MIT",
    package: "souvlaki",
    package_licence: "MIT",
}];
pub const CREDITS_SYSTEM: &[crate::about::Credit] = &[crate::about::Credit {
    name: "windows-sys",
    what: "Windows API bindings",
    licence: "MIT or Apache-2.0",
    package: "windows-sys",
    package_licence: "MIT OR Apache-2.0",
}];
/// What wgpu drives under Skia here.
pub const GPU_API_UNDER_SKIA: &str = "Direct3D 12 under Skia";
/// The target About's credits test checks the dependencies of.
#[cfg_attr(not(test), allow(dead_code))]
pub const TARGET_TRIPLE: &str = "x86_64-pc-windows-msvc";

/// The words that name the system in the UI.
pub const WORDS: crate::platform::Words = crate::platform::Words {
    tagline: "A local-first, artwork-forward music player for Windows.",
    show_in: "Show in Explorer",
    show_file: "Show the file in Explorer",
    show_folder: "Show the album's folder in Explorer",
    open_folder_in: " in Explorer",
    copy_names: "Windows",
    copy_label: "Copy the version, Windows version and renderer, for a bug report",
    default_output: "The system default follows Windows' choice of output, also when it changes.",
    exclusive_title: "Exclusive mode",
    exclusive_hint: "Bit-perfect, without the Windows mixer. Other apps go silent while it plays.",
    exclusive_more: "The device plays each track at its own sample rate and bit depth. It is let go a few seconds into a pause and when playback stops. Volume and ReplayGain still apply, and the equalizer while it is on; a track a crossfade brings in at another rate is converted to the rate playing until the next pause (the technical details say each).",
    profile: "user profile (AppData)",
    autostart: "Start with Windows",
    autostart_more: "Windows' Startup apps (Settings > Apps > Startup, or Task Manager) can turn it off too; turned off there, it shows off here.",
};

pub fn words() -> crate::platform::Words {
    WORDS
}

/// This process's memory (private bytes and peak working set, in MB) and open handles (the
/// bench's long runs watch them for growth).
#[cfg(feature = "bench")]
pub fn resources() -> (f64, f64, u32) {
    use windows_sys::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessHandleCount};
    // SAFETY: plain out-parameters on this process's pseudo handle, sized as declared.
    unsafe {
        let mut m: PROCESS_MEMORY_COUNTERS_EX = std::mem::zeroed();
        m.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
        let mut handles = 0u32;
        GetProcessHandleCount(GetCurrentProcess(), &mut handles);
        if GetProcessMemoryInfo(GetCurrentProcess(), (&raw mut m).cast(), m.cb) == 0 {
            return (0.0, 0.0, handles);
        }
        (m.PrivateUsage as f64 / 1_048_576.0, m.PeakWorkingSetSize as f64 / 1_048_576.0, handles)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_names_follow_the_build() {
        assert_eq!(windows_name(10, 0, 26100, "24H2", Some(2605)), "Windows 11 24H2 (build 26100.2605)");
        assert_eq!(windows_name(10, 0, 19045, "22H2", Some(5131)), "Windows 10 22H2 (build 19045.5131)");
        assert_eq!(windows_name(10, 0, 22000, "", None), "Windows 11 (build 22000)");
        assert!(os_version().starts_with("Windows"), "{}", os_version());
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
}
