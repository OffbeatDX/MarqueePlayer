//! Seek latency on the real output device, at volume 0 (silent).
//! `cargo run -p player-core --release --example seek_latency -- <file> [seeks]`
//! Prints, per seek and as medians, the time from the seek call to: the file being
//! repositioned, the first chunk from the new position decoded, its first samples
//! written to the device, and heard (written + device latency).
use std::time::Duration;

use player_core::{Player, TrackRequest};

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

fn main() {
    let mut args = std::env::args().skip(1);
    let path = std::path::PathBuf::from(args.next().expect("file"));
    let n: usize = args.next().and_then(|v| v.parse().ok()).unwrap_or(20);
    let (player, _events) = Player::new();
    player.set_volume(0.0);
    player.play(TrackRequest::new(1, path.clone()));
    std::thread::sleep(Duration::from_millis(1500));
    let duration = player.status().duration.unwrap_or(60.0);
    let mut rows = Vec::new();
    let mut x: u64 = 0x2545_f491;
    for i in 0..n {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        let to = (x % 1000) as f64 / 1000.0 * (duration - 5.0).max(1.0);
        player.seek(to);
        std::thread::sleep(Duration::from_millis(700));
        match player.status().last_seek {
            Some(t) => {
                println!(
                    "seek {i:2} to {to:7.2} s: positioned {:6.1}  decoded {:6.1}  written {:6.1}  heard {:6.1} ms",
                    t.positioned_ms, t.decoded_ms, t.written_ms, t.audible_ms()
                );
                rows.push(t);
            }
            None => println!("seek {i:2}: no timing"),
        }
    }
    if rows.is_empty() {
        return;
    }
    println!(
        "median of {}: positioned {:.1}  decoded {:.1}  written {:.1}  device {:.1}  heard {:.1} ms   ({})",
        rows.len(),
        median(rows.iter().map(|t| t.positioned_ms).collect()),
        median(rows.iter().map(|t| t.decoded_ms).collect()),
        median(rows.iter().map(|t| t.written_ms).collect()),
        median(rows.iter().map(|t| t.device_ms).collect()),
        median(rows.iter().map(|t| t.audible_ms()).collect()),
        path.extension().and_then(|e| e.to_str()).unwrap_or("?"),
    );
}
