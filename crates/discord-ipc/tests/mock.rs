//! The presence thread against a mock of the Discord app (its frames over a loopback
//! socket instead of the pipe; nothing reaches a real Discord): the handshake, an activity
//! set and cleared, Discord arriving late and going away, an unknown application ID, and
//! the limit on how often an activity is sent.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crossbeam_channel::{Receiver, unbounded};
use discord_ipc::{Activity, Options, Pipe, Presence, State};
use serde_json::{Value, json};

const APP: &str = "1234567890123456789";

fn read(conn: &mut TcpStream) -> Option<(u32, Value)> {
    let mut head = [0u8; 8];
    conn.read_exact(&mut head).ok()?;
    let mut body = vec![0u8; u32::from_le_bytes(head[4..].try_into().unwrap()) as usize];
    conn.read_exact(&mut body).ok()?;
    Some((u32::from_le_bytes(head[..4].try_into().unwrap()), serde_json::from_slice(&body).unwrap()))
}

fn write(conn: &mut TcpStream, op: u32, v: &Value) {
    let body = v.to_string();
    let mut frame = op.to_le_bytes().to_vec();
    frame.extend_from_slice(&(body.len() as u32).to_le_bytes());
    frame.extend_from_slice(body.as_bytes());
    let _ = conn.write_all(&frame);
}

struct Mock {
    /// Discord is running: off, the pipe is not there.
    up: Arc<AtomicBool>,
    /// Drop the connection at the next frame (Discord quit).
    drop_next: Arc<AtomicBool>,
    /// Each activity Discord was told to show (`null`: none).
    activities: Receiver<Value>,
    states: Receiver<State>,
    presence: Presence,
}

fn start(burst: usize, window: Duration) -> Mock {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let up = Arc::new(AtomicBool::new(true));
    let drop_next = Arc::new(AtomicBool::new(false));
    let (atx, activities) = unbounded();
    let dn = drop_next.clone();
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut conn) = conn else { continue };
            let Some((op, hello)) = read(&mut conn) else { continue };
            assert_eq!((op, &hello["v"]), (0, &json!(1)));
            if hello["client_id"] != APP {
                write(&mut conn, 2, &json!({ "code": 4000, "message": "Invalid Client ID" }));
                continue;
            }
            // A ping first: answered, and not taken for READY.
            write(&mut conn, 3, &json!({ "n": 1 }));
            write(&mut conn, 1, &json!({ "cmd": "DISPATCH", "evt": "READY", "data": { "v": 1, "user": { "username": "alice", "global_name": "Alice" } } }));
            assert_eq!(read(&mut conn), Some((4, json!({ "n": 1 }))));
            while let Some((op, v)) = read(&mut conn) {
                if dn.swap(false, Ordering::SeqCst) {
                    break;
                }
                assert_eq!((op, &v["cmd"]), (1, &json!("SET_ACTIVITY")));
                assert_eq!(v["args"]["pid"], json!(std::process::id()));
                let _ = atx.send(v["args"]["activity"].clone());
                write(&mut conn, 1, &json!({ "cmd": "SET_ACTIVITY", "evt": null, "nonce": v["nonce"] }));
            }
        }
    });
    let (stx, states) = unbounded();
    let running = up.clone();
    let connect = Box::new(move || -> std::io::Result<Box<dyn Pipe>> {
        if !running.load(Ordering::SeqCst) {
            return Err(std::io::Error::new(std::io::ErrorKind::NotFound, "not running"));
        }
        Ok(Box::new(TcpStream::connect(addr)?))
    });
    let presence = Presence::start(
        Options { connect, retry: Duration::from_millis(50), burst, window },
        Box::new(move |s| {
            let _ = stx.send(s);
        }),
    );
    Mock { up, drop_next, activities, states, presence }
}

const WAIT: Duration = Duration::from_secs(5);

fn track(title: &str) -> Activity {
    Activity { details: title.into(), state: "A Band".into(), start_ms: 1_700_000_000_000, end_ms: 1_700_000_245_000 }
}

#[test]
fn shows_and_clears_what_plays() {
    let m = start(10, Duration::from_secs(20));
    m.presence.configure(true, APP);
    assert_eq!(m.states.recv_timeout(WAIT), Ok(State::Connected { user: "Alice".into() }));
    m.presence.set_activity(Some(track("Lantern")));
    let shown = m.activities.recv_timeout(WAIT).unwrap();
    assert_eq!(shown, json!({ "type": 2, "status_display_type": 2, "details": "Lantern", "state": "A Band", "timestamps": { "start": 1_700_000_000_000i64, "end": 1_700_000_245_000i64 }, "instance": false }));
    // Paused: cleared.
    m.presence.set_activity(None);
    assert_eq!(m.activities.recv_timeout(WAIT), Ok(Value::Null));
    // The same again is not sent again.
    m.presence.set_activity(None);
    assert!(m.activities.recv_timeout(Duration::from_millis(200)).is_err());
    // Turned off: the pipe is closed, which is what clears the profile.
    m.presence.configure(false, APP);
    assert_eq!(m.states.recv_timeout(WAIT), Ok(State::Off));
}

#[test]
fn waits_for_discord_and_comes_back_after_it_restarts() {
    let m = start(10, Duration::from_secs(20));
    m.up.store(false, Ordering::SeqCst);
    m.presence.configure(true, APP);
    m.presence.set_activity(Some(track("Lantern")));
    assert_eq!(m.states.recv_timeout(WAIT), Ok(State::Waiting));
    // Discord starts: what plays is shown without being asked again.
    m.up.store(true, Ordering::SeqCst);
    assert_eq!(m.states.recv_timeout(WAIT), Ok(State::Connected { user: "Alice".into() }));
    assert_eq!(m.activities.recv_timeout(WAIT).unwrap()["details"], "Lantern");
    // Discord quits: noticed at the next track, which is shown once it is back.
    m.drop_next.store(true, Ordering::SeqCst);
    m.presence.set_activity(Some(track("Harbour")));
    assert_eq!(m.states.recv_timeout(WAIT), Ok(State::Waiting));
    assert_eq!(m.states.recv_timeout(WAIT), Ok(State::Connected { user: "Alice".into() }));
    assert_eq!(m.activities.recv_timeout(WAIT).unwrap()["details"], "Harbour");
}

#[test]
fn says_when_the_application_id_is_wrong() {
    let m = start(10, Duration::from_secs(20));
    m.presence.configure(true, "");
    assert_eq!(m.states.recv_timeout(WAIT), Ok(State::NoId));
    m.presence.configure(true, "not a number");
    assert!(matches!(m.states.recv_timeout(WAIT), Ok(State::Refused(why)) if why.contains("digits")));
    // A number Discord does not know: said, and not tried over and over.
    m.presence.configure(true, "9999999999999999999");
    assert!(matches!(m.states.recv_timeout(WAIT), Ok(State::Refused(why)) if why.contains("does not know")));
    assert!(m.states.recv_timeout(Duration::from_millis(300)).is_err());
    // Corrected: connects.
    m.presence.configure(true, APP);
    assert_eq!(m.states.recv_timeout(WAIT), Ok(State::Connected { user: "Alice".into() }));
}

#[test]
fn skipping_through_tracks_sends_only_what_discord_takes() {
    let window = Duration::from_millis(600);
    let m = start(2, window);
    m.presence.configure(true, APP);
    assert_eq!(m.states.recv_timeout(WAIT), Ok(State::Connected { user: "Alice".into() }));
    let began = std::time::Instant::now();
    for title in ["One", "Two"] {
        m.presence.set_activity(Some(track(title)));
        assert_eq!(m.activities.recv_timeout(WAIT).unwrap()["details"], title);
    }
    // Past the limit: the ones skipped over are never sent, the one that stays is, later.
    for title in ["Three", "Four", "Five"] {
        m.presence.set_activity(Some(track(title)));
    }
    assert_eq!(m.activities.recv_timeout(WAIT).unwrap()["details"], "Five");
    assert!(began.elapsed() >= window, "sent {:?} after the first", began.elapsed());
}
