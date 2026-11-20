//! `Event::Ended` belongs to the generation that ended. When a queue ends, the output lets what
//! the device holds play out before it says so; a track started in that moment (the app's next
//! play, a user's click) used to get that stale `Ended`, and the app, which moves on at
//! `Ended`, skipped the track it had just started. Caught by the `skip-damaged` bench on Linux,
//! where the device buffer is 45 ms and the bench's second play can land in it.

mod common;

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use common::{Capture, dir, write_wav};
use player_core::{Event, Player, TrackRequest};

#[test]
fn a_track_started_while_the_last_one_plays_out_is_not_ended_by_it() {
    let d = dir("ended-fixtures");
    let tone = |secs: f64, f: f64| -> Vec<f32> {
        (0..(48_000.0 * secs) as usize).flat_map(|i| [(0.3 * (std::f64::consts::TAU * f * i as f64 / 48_000.0).sin()) as f32; 2]).collect()
    };
    let (a, b) = (d.join("a.wav"), d.join("b.wav"));
    write_wav(&a, 48_000, &tone(0.3, 440.0));
    let b_samples = tone(1.0, 660.0);
    write_wav(&b, 48_000, &b_samples);
    let cap = Capture::default();
    // The device always says it holds audio not heard yet: at the end of the queue the output
    // waits for it to play out (up to its 2 s limit), and the test acts in that wait.
    cap.queued.store(4800, Ordering::Relaxed);
    let dev = cap.clone();
    let (player, events) = Player::with_device(move || Box::new(dev));
    player.play(TrackRequest::new(0, a));
    let a_frames = 48_000 * 3 / 10;
    let deadline = Instant::now() + Duration::from_secs(20);
    while cap.frames() < a_frames {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
    // A has been written out; the output is waiting for it to be heard.
    std::thread::sleep(Duration::from_millis(100));
    player.play(TrackRequest::new(1, b));
    std::thread::sleep(Duration::from_millis(100));
    cap.queued.store(0, Ordering::Relaxed);
    let mut seen = Vec::new();
    loop {
        match events.recv_timeout(Duration::from_secs(10)).expect("an end") {
            Event::TrackStarted { token } => seen.push(format!("started {token}")),
            Event::Ended => {
                seen.push("ended".to_owned());
                break;
            }
            Event::Error { message, .. } => panic!("{message}"),
            _ => {}
        }
    }
    drop(player);
    assert_eq!(seen, ["started 0", "started 1", "ended"], "one end, B's own");
    let got = cap.take();
    assert!(got[a_frames * 2..] == b_samples[..], "B whole after A");
}
