//! A stop is said `Stopped` and stays so. The output thread wrote the state it was in after
//! the decode thread had reset it: a stop while playing (the output in the device's wait) came
//! back as `Playing` with the old token, a stop while paused as `Paused` at every later wake,
//! and the app's next Play press only toggled the pause.

mod common;

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use common::{Capture, dir, write_wav};
use player_core::{PlayState, Player, TrackRequest};

fn tone() -> Vec<f32> {
    (0..48_000 * 5).flat_map(|i| [(0.3 * (std::f64::consts::TAU * 440.0 * i as f64 / 48_000.0).sin()) as f32; 2]).collect()
}

fn playing(cap: &Capture, name: &str) -> Player {
    let d = dir("stopped-fixtures");
    let path = d.join(name);
    write_wav(&path, 48_000, &tone());
    let dev = cap.clone();
    let (player, _events) = Player::with_device(move || Box::new(dev));
    player.play(TrackRequest::new(3, path));
    let deadline = Instant::now() + Duration::from_secs(20);
    while !(player.status().state == PlayState::Playing && cap.frames() > 4800) {
        assert!(Instant::now() < deadline, "playing");
        std::thread::sleep(Duration::from_millis(1));
    }
    player
}

#[test]
fn a_stop_while_playing_stays_stopped() {
    let cap = Capture::paced();
    let player = playing(&cap, "a.wav");
    // The output is in the device's wait when the stop comes, and writes once more after it.
    cap.hold.store(true, Ordering::Release);
    std::thread::sleep(Duration::from_millis(30));
    player.stop();
    std::thread::sleep(Duration::from_millis(200));
    cap.hold.store(false, Ordering::Release);
    std::thread::sleep(Duration::from_millis(300));
    let st = player.status();
    assert_eq!(st.state, PlayState::Stopped, "{st:?}");
    assert_eq!(st.token, None, "{st:?}");
}

#[test]
fn a_stop_while_paused_stays_stopped() {
    let cap = Capture::paced();
    let player = playing(&cap, "b.wav");
    player.set_paused(true);
    let deadline = Instant::now() + Duration::from_secs(5);
    while player.status().state != PlayState::Paused {
        assert!(Instant::now() < deadline, "paused");
        std::thread::sleep(Duration::from_millis(1));
    }
    player.stop();
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(player.status().state, PlayState::Stopped);
    // A later wake of the output (the system's devices changed) leaves it so.
    player.devices_changed();
    std::thread::sleep(Duration::from_millis(200));
    let st = player.status();
    assert_eq!(st.state, PlayState::Stopped, "{st:?}");
    assert_eq!(st.token, None, "{st:?}");
}
