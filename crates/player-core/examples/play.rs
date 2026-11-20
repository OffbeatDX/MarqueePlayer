//! Play files one after another, gapless, on the system's output (WASAPI, PipeWire), then
//! exit: for checking an output by ear or by recording it.
//!
//!     cargo run --release -p player-core --example play -- a.flac b.flac
//!
//! `PLAY_VOLUME` (0 to 1, default 1) sets the volume; `PLAY_PAUSE_AT=<s>` pauses for a second
//! that far into the first file and resumes; `PLAY_SEEK_AT=<s>` seeks back one second there.
//! `PLAY_DEVICE=<id>` plays on that device (as Settings > Output chooses it), `PLAY_EXCLUSIVE=1`
//! asks for exclusive mode (Windows) or the bit-perfect rate (Linux).

use std::time::{Duration, Instant};

struct Stderr;
impl log::Log for Stderr {
    fn enabled(&self, _: &log::Metadata) -> bool {
        true
    }
    fn log(&self, r: &log::Record) {
        eprintln!("[{}] {}", r.target(), r.args());
    }
    fn flush(&self) {}
}
static STDERR: Stderr = Stderr;

use player_core::{Event, GainConfig, GainMode, OutputConfig, Player, TrackRequest};

fn main() {
    if std::env::var_os("PLAY_LOG").is_some() {
        log::set_max_level(log::LevelFilter::Info);
        let _ = log::set_logger(&STDERR);
    }
    let files: Vec<String> = std::env::args().skip(1).collect();
    if files.is_empty() {
        eprintln!("usage: play <file>...");
        std::process::exit(2);
    }
    let env_secs = |k: &str| std::env::var(k).ok().and_then(|v| v.parse::<f64>().ok());
    let (player, events) = Player::new();
    player.set_gain_config(GainConfig { mode: GainMode::Off, limit_peaks: false, ..Default::default() });
    player.set_volume(env_secs("PLAY_VOLUME").unwrap_or(1.0) as f32);
    player.set_output(OutputConfig { device: std::env::var("PLAY_DEVICE").ok(), exclusive: std::env::var("PLAY_EXCLUSIVE").is_ok_and(|v| v == "1") });
    player.play(TrackRequest::new(0, files[0].clone()));
    let (mut next, mut started) = (1usize, Instant::now());
    let (mut paused, mut sought) = (env_secs("PLAY_PAUSE_AT"), env_secs("PLAY_SEEK_AT"));
    loop {
        match events.recv_timeout(Duration::from_millis(20)) {
            Ok(Event::TrackStarted { token }) => {
                eprintln!("started {} ({})", files[token as usize], player.status().position);
                if token == 0 {
                    started = Instant::now();
                }
                if next < files.len() {
                    player.set_next(Some(TrackRequest::new(next as u64, files[next].clone())));
                    next += 1;
                }
            }
            Ok(Event::Output(info)) => eprintln!("output: {info:?}"),
            Ok(Event::NoOutput(why)) => eprintln!("no output: {why:?}"),
            Ok(Event::Ended) => break,
            Ok(Event::Error { message, .. }) => {
                eprintln!("error: {message}");
                std::process::exit(1);
            }
            _ => {}
        }
        let t = started.elapsed().as_secs_f64();
        if paused.is_some_and(|at| t >= at) {
            paused = None;
            player.set_paused(true);
            std::thread::sleep(Duration::from_secs(1));
            player.set_paused(false);
            eprintln!("paused for 1 s at {t:.2} s");
        }
        if sought.is_some_and(|at| t >= at) {
            sought = None;
            let p = player.status().position;
            player.seek((p - 1.0).max(0.0));
            eprintln!("seek from {p:.3} s back to {:.3} s", (p - 1.0).max(0.0));
        }
    }
    eprintln!("ended; output {:?}", player.output_info());
}
