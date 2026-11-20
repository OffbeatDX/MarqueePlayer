//! A next track whose open hangs (a network share gone away, a disk spinning up) is not waited
//! for with the commands left unread: the decode thread waited on `mp-open` at the end of the
//! track before it, and a stop, a play of another track and closing the app all waited for
//! that open to fail. Windows only: the hanging file is a named pipe that never writes.

#![cfg(windows)]

mod common;

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use common::{Capture, dir, write_wav};
use player_core::{Event, PlayState, Player, TrackRequest};

/// A pipe server that takes the connection and then sends nothing for a minute: a read of it
/// waits.
struct Hang(Child);

impl Hang {
    fn start(name: &str) -> Self {
        let script = format!(
            "$p = New-Object System.IO.Pipes.NamedPipeServerStream('{name}'); [Console]::Out.WriteLine('ready'); [Console]::Out.Flush(); $p.WaitForConnection(); Start-Sleep 60"
        );
        let mut child = Command::new("powershell").args(["-NoProfile", "-NonInteractive", "-Command", &script]).stdout(Stdio::piped()).spawn().unwrap();
        let mut line = String::new();
        BufReader::new(child.stdout.as_mut().unwrap()).read_line(&mut line).unwrap();
        assert_eq!(line.trim(), "ready");
        Self(child)
    }
}

impl Drop for Hang {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn a_next_track_whose_open_hangs_does_not_hold_up_the_commands() {
    let d = dir("slow-open-fixtures");
    let tone = |secs: f64| -> Vec<f32> {
        (0..(48_000.0 * secs) as usize).flat_map(|i| [(0.3 * (std::f64::consts::TAU * 440.0 * i as f64 / 48_000.0).sin()) as f32; 2]).collect()
    };
    let (a, b) = (d.join("a.wav"), d.join("b.wav"));
    write_wav(&a, 48_000, &tone(0.3));
    write_wav(&b, 48_000, &tone(0.5));
    let name = format!("mp-slow-open-{}.wav", std::process::id());
    let _hang = Hang::start(&name);

    let cap = Capture::paced();
    let dev = cap.clone();
    let (player, events) = Player::with_device(move || Box::new(dev));
    player.play(TrackRequest::new(0, a));
    player.set_next(Some(TrackRequest::new(1, format!(r"\\.\pipe\{name}"))));
    // A plays to its end; its successor is still opening.
    let deadline = Instant::now() + Duration::from_secs(20);
    while cap.frames() < 48_000 * 3 / 10 {
        assert!(Instant::now() < deadline, "A plays out");
        std::thread::sleep(Duration::from_millis(1));
    }
    std::thread::sleep(Duration::from_millis(100));

    // The user picks another track: it starts.
    player.play(TrackRequest::new(2, b));
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match events.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(Event::TrackStarted { token: 2 }) => break,
            Ok(_) => {}
            Err(_) => panic!("the play waited for the hanging open"),
        }
    }
    // The stop is taken at once too, and the engine closes.
    player.set_next(Some(TrackRequest::new(3, format!(r"\\.\pipe\{name}"))));
    player.stop();
    let deadline = Instant::now() + Duration::from_secs(2);
    while player.status().state != PlayState::Stopped {
        assert!(Instant::now() < deadline, "stopped");
        std::thread::sleep(Duration::from_millis(1));
    }
    let at = Instant::now();
    drop(player);
    assert!(at.elapsed() < Duration::from_secs(2), "the engine closes: {:?}", at.elapsed());
}
