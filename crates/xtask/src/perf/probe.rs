//! What `cargo xtask perf` reads from outside the app: the window server's CPU time, the GPU's
//! utilisation, the watts of `powermetrics` and of the battery's controller, whether the
//! screen is locked; and what it holds while measuring: the display awake, the clock up.
//! macOS only, behind `cfg`: elsewhere each reader says it reads nothing, and the comparison
//! has the app's own figures alone. The parsers are plain functions of the tools' text, tested
//! on samples of their real output.

use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// What this system's readers cannot see, for the report.
pub fn not_read_here() -> Vec<&'static str> {
    if cfg!(target_os = "macos") {
        Vec::new()
    } else {
        vec!["the window server's (compositor's) CPU", "the GPU's utilisation", "watts", "whether the screen is locked"]
    }
}

// ---- The window server's CPU time ----------------------------------------------------------

/// The window server's process id (macOS: `WindowServer`).
pub fn window_server() -> Option<u32> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let out = Command::new("pgrep").args(["-x", "WindowServer"]).stdin(Stdio::null()).output().ok()?;
    String::from_utf8_lossy(&out.stdout).lines().next()?.trim().parse().ok()
}

/// A process's CPU time so far, in seconds, by `ps` (which reads it for any user's process,
/// to a hundredth of a second).
pub fn cpu_seconds(pid: u32) -> Option<f64> {
    let out = Command::new("ps").args(["-o", "time=", "-p", &pid.to_string()]).stdin(Stdio::null()).output().ok()?;
    parse_ps_time(String::from_utf8_lossy(&out.stdout).trim())
}

/// `ps -o time=` on macOS: `[[dd-]hh:]mm:ss.cc`, the minutes unbounded without hours
/// (`104:46.89`).
pub fn parse_ps_time(s: &str) -> Option<f64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let (days, rest) = match s.split_once('-') {
        Some((d, r)) => (d.parse::<f64>().ok()?, r),
        None => (0.0, s),
    };
    let parts: Vec<&str> = rest.split(':').collect();
    let mut secs = 0.0;
    for p in &parts {
        secs = secs * 60.0 + p.parse::<f64>().ok()?;
    }
    if parts.len() > 3 {
        return None;
    }
    Some(days * 86_400.0 + secs)
}

// ---- The whole machine's CPU ----------------------------------------------------------------

/// Starts `iostat` to read the whole machine's CPU over the next `secs` seconds (its second
/// line is the mean over that interval; the first is since boot). Unix only.
pub fn start_machine_cpu(secs: u64) -> Option<Child> {
    if !cfg!(unix) {
        return None;
    }
    Command::new("iostat").args(["-n0", "-w", &secs.max(1).to_string(), "-c", "2"]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()
}

/// The cores busy (user and system) over the interval, from `iostat -n0 -w N -c 2`'s last
/// line (`us sy id` as shares of every core, then the load averages).
pub fn parse_iostat(text: &str, cores: usize) -> Option<f64> {
    let line = text.lines().rev().find(|l| l.split_whitespace().next().is_some_and(|w| w.parse::<f64>().is_ok()))?;
    let f: Vec<f64> = line.split_whitespace().filter_map(|w| w.parse().ok()).collect();
    let idle = *f.get(2)?;
    Some((100.0 - idle) / 100.0 * cores as f64)
}

// ---- The GPU's utilisation -------------------------------------------------------------------

/// The GPU's "Device Utilization %" now (macOS: `ioreg`, the accelerator's performance
/// statistics, which the driver refreshes about once a second).
pub fn gpu_utilisation() -> Option<f64> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let out = Command::new("ioreg").args(["-r", "-d", "1", "-c", "IOAccelerator"]).stdin(Stdio::null()).output().ok()?;
    parse_gpu_utilisation(&String::from_utf8_lossy(&out.stdout))
}

pub fn parse_gpu_utilisation(text: &str) -> Option<f64> {
    let key = "\"Device Utilization %\"=";
    let at = text.find(key)? + key.len();
    let digits: String = text[at..].chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

/// Samples the GPU once a second until stopped; the samples when joined.
pub struct GpuSampler {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<Vec<f64>>>,
}

impl GpuSampler {
    pub fn start() -> Option<Self> {
        if !cfg!(target_os = "macos") {
            return None;
        }
        let stop = Arc::new(AtomicBool::new(false));
        let s = stop.clone();
        let thread = std::thread::spawn(move || {
            let mut v = Vec::new();
            let start = std::time::Instant::now();
            let mut k = 0u32;
            while !s.load(Ordering::Relaxed) {
                if let Some(u) = gpu_utilisation() {
                    v.push(u);
                }
                k += 1;
                // Once a second from the start, however long ioreg took.
                while !s.load(Ordering::Relaxed) && start.elapsed().as_secs_f64() < f64::from(k) {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
            }
            v
        });
        Some(Self { stop, thread: Some(thread) })
    }

    pub fn finish(mut self) -> Vec<f64> {
        self.stop.store(true, Ordering::Relaxed);
        self.thread.take().and_then(|t| t.join().ok()).unwrap_or_default()
    }
}

// ---- Watts: powermetrics --------------------------------------------------------------------

/// The one `powermetrics` command the owner's sudoers rule allows without a password: ten
/// samples a second apart.
pub const POWERMETRICS: [&str; 8] = ["-n", "/usr/bin/powermetrics", "--samplers", "cpu_power,gpu_power", "-i", "1000", "-n", "10"];

/// Whether `sudo -n` runs `powermetrics` (never asks for a password: `-n` fails instead).
pub fn powermetrics_allowed() -> bool {
    if !cfg!(target_os = "macos") {
        return false;
    }
    // `sudo -n -l <command>` says whether the command would run without a password, running
    // nothing.
    let mut args = vec!["-n", "-l"];
    args.extend(&POWERMETRICS[1..]);
    Command::new("sudo").args(&args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
}

pub fn start_powermetrics() -> Option<Child> {
    Command::new("sudo").args(POWERMETRICS).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()
}

/// The CPU's and the GPU's watts of each sample in `powermetrics --samplers
/// cpu_power,gpu_power` text: its lines `CPU Power: 1234 mW` and `GPU Power: 56 mW`
/// (the GPU section repeats the GPU's line: one each a sample).
pub fn parse_powermetrics(text: &str) -> (Vec<f64>, Vec<f64>) {
    let (mut cpu, mut gpu) = (Vec::new(), Vec::new());
    let mut gpu_this_sample = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with("*** Sampled system activity") {
            gpu_this_sample = false;
        }
        let mw = |prefix: &str| line.strip_prefix(prefix).and_then(|r| r.trim().strip_suffix("mW")).and_then(|n| n.trim().parse::<f64>().ok());
        if let Some(v) = mw("CPU Power:") {
            cpu.push(v / 1000.0);
        } else if let Some(v) = mw("GPU Power:")
            && !gpu_this_sample
        {
            gpu.push(v / 1000.0);
            gpu_this_sample = true;
        }
    }
    (cpu, gpu)
}

// ---- Watts: the battery's controller --------------------------------------------------------

/// The battery controller's running sum of the whole machine's power (mW, one sample a
/// second) and the count of samples in it: `ioreg -rw0 -c AppleSmartBattery`,
/// "PowerTelemetryData", "AccumulatedSystemLoad" and "SystemLoadAccumulatorCount". It moves
/// about once a minute.
pub fn battery_accumulator() -> Option<(u64, u64)> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let out = Command::new("ioreg").args(["-rw0", "-c", "AppleSmartBattery"]).stdin(Stdio::null()).output().ok()?;
    parse_battery(&String::from_utf8_lossy(&out.stdout))
}

pub fn parse_battery(text: &str) -> Option<(u64, u64)> {
    let tel = &text[text.find("\"PowerTelemetryData\"")?..];
    let num = |key: &str| -> Option<u64> {
        let k = format!("\"{key}\"=");
        let at = tel.find(&k)? + k.len();
        tel[at..].chars().take_while(|c| c.is_ascii_digit()).collect::<String>().parse().ok()
    };
    Some((num("AccumulatedSystemLoad")?, num("SystemLoadAccumulatorCount")?))
}

/// The mean power (W) between two readings of the accumulator.
pub fn battery_watts(a: (u64, u64), b: (u64, u64)) -> Option<f64> {
    let (de, dn) = (b.0.checked_sub(a.0)?, b.1.checked_sub(a.1)?);
    (dn > 0).then(|| de as f64 / dn as f64 / 1000.0)
}

// ---- The screen, the display, the clock -----------------------------------------------------

/// Whether the screen is locked (nothing draws then): macOS's `IOConsoleLocked`. False
/// where it cannot be read.
pub fn screen_locked() -> bool {
    if !cfg!(target_os = "macos") {
        return false;
    }
    Command::new("ioreg").args(["-n", "Root", "-d1"]).stdin(Stdio::null()).output().is_ok_and(|o| parse_locked(&String::from_utf8_lossy(&o.stdout)))
}

pub fn parse_locked(text: &str) -> bool {
    text.contains("\"IOConsoleLocked\" = Yes")
}

/// Held while measuring, let go when dropped: the display kept awake (macOS: `caffeinate -d`,
/// which also ends with this process), and with `busy`, two processes keeping two cores busy
/// so the clock stays up and a CPU time counts work, not the time light work takes at a low
/// clock (docs/testing.md, "Why with the clock held").
pub struct Hold {
    children: Vec<Child>,
    stop: Arc<AtomicBool>,
}

impl Hold {
    pub fn new(busy: bool) -> Self {
        let mut children = Vec::new();
        let stop = Arc::new(AtomicBool::new(false));
        if cfg!(target_os = "macos")
            && let Ok(c) = Command::new("caffeinate").args(["-d", "-w", &std::process::id().to_string()]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()
        {
            children.push(c);
        }
        if busy {
            for _ in 0..2 {
                if cfg!(unix)
                    && let Ok(c) = Command::new("yes").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()
                {
                    children.push(c);
                    continue;
                }
                let s = stop.clone();
                std::thread::spawn(move || {
                    let mut x = 0u64;
                    while !s.load(Ordering::Relaxed) {
                        x = std::hint::black_box(x.wrapping_add(1));
                    }
                });
            }
        }
        Self { children, stop }
    }
}

impl Drop for Hold {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        for c in &mut self.children {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ps_times_read_with_and_without_hours() {
        // As `ps -o time= -p <WindowServer>` printed it on 2026-11-26.
        assert_eq!(parse_ps_time("104:46.89"), Some(104.0 * 60.0 + 46.89));
        assert_eq!(parse_ps_time("  0:00.12 "), Some(0.12));
        assert_eq!(parse_ps_time("1:02:03.50"), Some(3723.5));
        assert_eq!(parse_ps_time("2-01:00:00.00"), Some(2.0 * 86_400.0 + 3600.0));
        assert_eq!(parse_ps_time(""), None);
        assert_eq!(parse_ps_time("n/a"), None);
    }

    #[test]
    fn the_gpu_utilisation_is_read_from_ioreg() {
        // From `ioreg -r -d 1 -c IOAccelerator` on the M5 MacBook Air, 2026-11-26 (one line).
        let text = r#"+-o AGXAcceleratorG17G  <class AGXAcceleratorG17G, id 0x10000042e, registered, matched, active, busy 0 (2501 ms), retain 43>
    {
      "PerformanceStatistics" = {"In use system memory (driver)"=0,"Alloc system memory"=1665400832,"Tiler Utilization %"=4,"recoveryCount"=0,"lastRecoveryTime"=0,"Renderer Utilization %"=15,"TiledSceneBytes"=950272,"Device Utilization %"=15,"SplitSceneCount"=0,"Allocated PB Size"=64487424,"In use system memory"=169885696}
    }"#;
        assert_eq!(parse_gpu_utilisation(text), Some(15.0));
        assert_eq!(parse_gpu_utilisation("nothing"), None);
    }

    #[test]
    fn the_battery_accumulator_gives_the_mean_power_between_two_readings() {
        // From `ioreg -rw0 -c AppleSmartBattery` on the M5 MacBook Air, 2026-11-26.
        let text = r#""PowerTelemetryData" = {"AccumulatedWallEnergyEstimate"=153522949,"SystemEnergyConsumed"=3855,"SystemPowerInAccumulatorCount"=48414,"AdapterEfficiencyLoss"=216,"SystemLoad"=13880,"AccumulatedSystemLoad"=507900597,"AccumulatedSystemEnergyConsumed"=41904355546,"SystemCurrentIn"=701,"WallEnergyEstimate"=4071,"AccumSystemEffectiveTotalLoadCount"=0,"SystemLoadAccumulatorCount"=82444,"AdapterEfficiencyLossAccumulatorCount"=48309}"#;
        let a = parse_battery(text).unwrap();
        assert_eq!(a, (507_900_597, 82_444));
        // A minute later at 6.5 W.
        let b = (a.0 + 60 * 6500, a.1 + 60);
        assert!((battery_watts(a, b).unwrap() - 6.5).abs() < 1e-9);
        assert_eq!(battery_watts(a, a), None);
    }

    #[test]
    fn powermetrics_gives_cpu_and_gpu_watts_a_sample() {
        // The layout of `powermetrics --samplers cpu_power,gpu_power` (macOS 15 and later):
        // written from Apple's documented format, not captured here (the sudoers rule was not
        // in place when this was written: check it against real output once it is).
        let text = "*** Sampled system activity (Wed Nov 26 18:00:00 2026 +0100) (1004.31ms elapsed) ***

**** Processor usage ****

E-Cluster HW active frequency: 1020 MHz
E-Cluster HW active residency:  23.10%
P-Cluster HW active frequency: 3000 MHz
CPU Power: 1234 mW
GPU Power: 56 mW
ANE Power: 0 mW
Combined Power (CPU + GPU + ANE): 1290 mW

**** GPU usage ****

GPU HW active frequency: 338 MHz
GPU HW active residency:  12.34%
GPU idle residency:  87.66%
GPU Power: 56 mW

*** Sampled system activity (Wed Nov 26 18:00:01 2026 +0100) (1001.02ms elapsed) ***

CPU Power: 1500 mW
GPU Power: 80 mW
Combined Power (CPU + GPU + ANE): 1580 mW

GPU Power: 80 mW
";
        let (cpu, gpu) = parse_powermetrics(text);
        assert_eq!(cpu, [1.234, 1.5]);
        assert_eq!(gpu, [0.056, 0.08]);
    }

    #[test]
    fn the_machines_busy_cores_come_from_iostats_second_line() {
        // `iostat -n0 -w 2 -c 2` on the M5 MacBook Air (10 cores), 2026-11-26.
        let text = "      cpu    load average\n us sy id   1m   5m   15m\n 13  5 82  7.81 5.39 3.85\n  6  3 91  7.81 5.39 3.85\n";
        assert!((parse_iostat(text, 10).unwrap() - 0.9).abs() < 1e-9);
        assert_eq!(parse_iostat("      cpu    load average\n", 10), None);
    }

    #[test]
    fn a_locked_screen_is_seen_in_ioreg() {
        assert!(parse_locked(r#"    "IOConsoleLocked" = Yes"#));
        assert!(!parse_locked(r#"    "IOConsoleUsers" = ({"kCGSSessionOnConsoleKey"=Yes})"#));
    }
}
