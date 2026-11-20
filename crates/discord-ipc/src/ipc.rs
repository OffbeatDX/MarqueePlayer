//! The Discord app's local RPC: a named pipe on Windows (`\\.\pipe\discord-ipc-0` to `-9`),
//! a Unix socket of the same name elsewhere. Each message is a frame: the opcode and the
//! payload's length (two little-endian u32), then JSON. The client says who it is (the
//! application ID), Discord answers READY, and `SET_ACTIVITY` then sets or clears what the
//! profile shows. Closing the pipe clears it too.

use std::io::{self, Read, Write};
use std::time::Duration;

use serde_json::{Value, json};

const HANDSHAKE: u32 = 0;
const FRAME: u32 = 1;
const CLOSE: u32 = 2;
const PING: u32 = 3;
const PONG: u32 = 4;

/// No payload of Discord's is anywhere near this; a length beyond it is not a frame.
const MAX_PAYLOAD: usize = 1 << 20;
/// Discord takes a text field of 2 to 128 characters.
const FIELD_MAX: usize = 128;

/// How long Discord may take over a frame, to answer one or to take one: past it the read or
/// write fails, and the pipe is closed and looked for again, as when it breaks.
pub const TIMEOUT: Duration = Duration::from_secs(10);

/// Discord's close code for an application ID it does not know.
const INVALID_CLIENT_ID: i64 = 4000;

/// The pipe to Discord: anything read and written in order.
pub trait Pipe: Read + Write + Send {}
impl<T: Read + Write + Send> Pipe for T {}

/// What the profile shows: "Listening to", the track and its artist, and how far in.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Activity {
    /// The first line: the track's title.
    pub details: String,
    /// The second line: its artist. Empty: no second line.
    pub state: String,
    /// When the track started, had it played without pause (Unix time, milliseconds).
    pub start_ms: i64,
    /// When it ends; 0 when its length is unknown (Discord then counts up).
    pub end_ms: i64,
}

#[derive(Debug)]
pub enum Error {
    /// Discord is not there, or the pipe broke.
    Io(io::Error),
    /// Discord does not know the application ID.
    InvalidId,
    /// Discord closed the connection, with its reason.
    Closed(String),
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "{e}"),
            Self::InvalidId => f.write_str("Discord does not know this application ID"),
            Self::Closed(why) => write!(f, "Discord closed the connection: {why}"),
        }
    }
}

/// A text field as Discord takes it: `None` when empty, cut to 128 characters, and a single
/// character padded to the two Discord asks for.
pub fn field(s: &str) -> Option<String> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let mut out: String = if s.chars().count() > FIELD_MAX { s.chars().take(FIELD_MAX - 1).chain(['\u{2026}']).collect() } else { s.to_owned() };
    if out.chars().count() < 2 {
        out.push(' ');
    }
    Some(out)
}

/// The activity as `SET_ACTIVITY` takes it. Type 2 is "Listening to".
pub fn activity_json(a: &Activity) -> Value {
    let mut timestamps = json!({ "start": a.start_ms });
    if a.end_ms > a.start_ms {
        timestamps["end"] = json!(a.end_ms);
    }
    // `status_display_type` 2: the one-line status under the name (the member list) shows
    // the title (`details`) instead of the application's name.
    let mut v = json!({ "type": 2, "status_display_type": 2, "timestamps": timestamps, "instance": false });
    if let Some(details) = field(&a.details) {
        v["details"] = json!(details);
    }
    if let Some(state) = field(&a.state) {
        v["state"] = json!(state);
    }
    v
}

fn write_frame(pipe: &mut dyn Pipe, op: u32, payload: &Value) -> io::Result<()> {
    let body = payload.to_string();
    // One write: a frame is one message on Windows' pipe.
    let mut frame = Vec::with_capacity(8 + body.len());
    frame.extend_from_slice(&op.to_le_bytes());
    frame.extend_from_slice(&(body.len() as u32).to_le_bytes());
    frame.extend_from_slice(body.as_bytes());
    pipe.write_all(&frame)?;
    pipe.flush()
}

fn read_frame(pipe: &mut dyn Pipe) -> io::Result<(u32, Value)> {
    let mut head = [0u8; 8];
    pipe.read_exact(&mut head)?;
    let op = u32::from_le_bytes([head[0], head[1], head[2], head[3]]);
    let len = u32::from_le_bytes([head[4], head[5], head[6], head[7]]) as usize;
    if len > MAX_PAYLOAD {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "a frame too large to be Discord's"));
    }
    let mut body = vec![0u8; len];
    pipe.read_exact(&mut body)?;
    Ok((op, serde_json::from_slice(&body).unwrap_or(Value::Null)))
}

/// A connection Discord has answered READY on.
pub struct Session {
    pipe: Box<dyn Pipe>,
    /// The Discord user signed in there, to show.
    pub user: String,
    nonce: u64,
}

impl Session {
    /// Say who we are and wait for Discord's READY.
    pub fn open(mut pipe: Box<dyn Pipe>, app_id: &str) -> Result<Self, Error> {
        write_frame(&mut *pipe, HANDSHAKE, &json!({ "v": 1, "client_id": app_id }))?;
        let ready = answer(&mut *pipe)?;
        let user = &ready["data"]["user"];
        let user = user["global_name"].as_str().filter(|n| !n.is_empty()).or_else(|| user["username"].as_str()).unwrap_or_default().to_owned();
        Ok(Self { pipe, user, nonce: 0 })
    }

    /// Show `activity`, or nothing.
    pub fn set_activity(&mut self, activity: Option<&Activity>) -> Result<(), Error> {
        self.nonce += 1;
        let payload = json!({
            "cmd": "SET_ACTIVITY",
            "args": { "pid": std::process::id(), "activity": activity.map(activity_json) },
            "nonce": self.nonce.to_string(),
        });
        write_frame(&mut *self.pipe, FRAME, &payload)?;
        let reply = answer(&mut *self.pipe)?;
        if reply["evt"] == "ERROR" {
            // Not worth dropping the connection for, and sending it again would not help.
            log::warn!("Discord did not take the activity: {}", reply["data"]["message"].as_str().unwrap_or("no reason given"));
        }
        Ok(())
    }
}

/// Discord's next answer, with its pings answered on the way.
fn answer(pipe: &mut dyn Pipe) -> Result<Value, Error> {
    loop {
        let (op, v) = read_frame(pipe)?;
        match op {
            FRAME => return Ok(v),
            PING => write_frame(pipe, PONG, &v)?,
            CLOSE if v["code"].as_i64() == Some(INVALID_CLIENT_ID) => return Err(Error::InvalidId),
            CLOSE => return Err(Error::Closed(v["message"].as_str().unwrap_or("no reason given").to_owned())),
            _ => {}
        }
    }
}

/// The Discord app's pipe on this machine: the first of its ten that opens.
pub fn system() -> io::Result<Box<dyn Pipe>> {
    #[cfg(windows)]
    for i in 0..10 {
        if let Ok(p) = win::Pipe::open(&format!(r"\\.\pipe\discord-ipc-{i}"), TIMEOUT) {
            return Ok(Box::new(p));
        }
    }
    #[cfg(unix)]
    {
        // The runtime folder, where the native package, Flatpak and Snap each put it on Linux;
        // on macOS the user's temporary folder (`$TMPDIR`, `/var/folders/.../T/`).
        let roots: Vec<std::path::PathBuf> = ["XDG_RUNTIME_DIR", "TMPDIR", "TMP", "TEMP"].iter().filter_map(std::env::var_os).map(Into::into).chain([std::path::PathBuf::from("/tmp")]).collect();
        for root in &roots {
            for sub in ["", "app/com.discordapp.Discord", "snap.discord", ".flatpak/com.discordapp.Discord/xdg-run"] {
                for i in 0..10 {
                    if let Ok(s) = unix(&root.join(sub).join(format!("discord-ipc-{i}")), TIMEOUT) {
                        return Ok(Box::new(s));
                    }
                }
            }
        }
    }
    Err(io::Error::new(io::ErrorKind::NotFound, "Discord is not running"))
}

/// Discord's socket, each read and write given up after `timeout`.
#[cfg(unix)]
fn unix(path: &std::path::Path, timeout: Duration) -> io::Result<std::os::unix::net::UnixStream> {
    let s = std::os::unix::net::UnixStream::connect(path)?;
    s.set_read_timeout(Some(timeout))?;
    s.set_write_timeout(Some(timeout))?;
    Ok(s)
}

/// Discord's named pipe, opened for overlapped I/O so that each read and write can give up: a
/// plain read of a pipe waits for ever when the other end never writes.
#[cfg(windows)]
mod win {
    use std::io;
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use std::time::Duration;

    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_IO_INCOMPLETE, ERROR_IO_PENDING, ERROR_MORE_DATA, HANDLE, WAIT_TIMEOUT};
    use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_OVERLAPPED, ReadFile, WriteFile};
    use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, GetOverlappedResultEx, OVERLAPPED};
    use windows_sys::Win32::System::Threading::CreateEventW;

    pub struct Pipe {
        file: std::fs::File,
        /// Signalled when an operation completes; ours alone.
        event: HANDLE,
        timeout_ms: u32,
    }

    // SAFETY: the event is a kernel handle, usable from any thread; only the owner uses it.
    unsafe impl Send for Pipe {}

    impl Pipe {
        pub fn open(path: &str, timeout: Duration) -> io::Result<Self> {
            let file = std::fs::OpenOptions::new().read(true).write(true).custom_flags(FILE_FLAG_OVERLAPPED).open(path)?;
            // SAFETY: no attributes and no name: a manual-reset event, closed in `drop`.
            let event = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
            if event.is_null() {
                return Err(io::Error::last_os_error());
            }
            Ok(Self { file, event, timeout_ms: timeout.as_millis().min(u128::from(u32::MAX - 1)) as u32 })
        }

        /// One read or write that `start` begins, waited for up to the timeout and cancelled
        /// past it. Returns only once the system is done with the buffer `start` was given.
        fn wait(&self, start: impl FnOnce(HANDLE, *mut OVERLAPPED) -> i32) -> io::Result<usize> {
            let h = self.file.as_raw_handle() as HANDLE;
            // SAFETY: OVERLAPPED is plain data, zeroed as an operation's start.
            let mut ov: OVERLAPPED = unsafe { std::mem::zeroed() };
            ov.hEvent = self.event;
            if start(h, &mut ov) == 0 {
                let e = io::Error::last_os_error();
                if e.raw_os_error() != Some(ERROR_IO_PENDING as i32) {
                    return Err(e);
                }
            }
            let mut n = 0u32;
            // SAFETY: `ov` outlives the operation: it is over when this returns true, and
            // waited for after the cancel below otherwise.
            if unsafe { GetOverlappedResultEx(h, &ov, &mut n, self.timeout_ms, 0) } != 0 {
                return Ok(n as usize);
            }
            let e = io::Error::last_os_error();
            match e.raw_os_error().map(|c| c as u32) {
                // A message longer than the buffer: the rest comes with the next read.
                Some(ERROR_MORE_DATA) => Ok(n as usize),
                Some(WAIT_TIMEOUT | ERROR_IO_INCOMPLETE) => {
                    // SAFETY: as above; waiting (TRUE) until the cancelled operation is over.
                    unsafe {
                        CancelIoEx(h, &ov);
                        GetOverlappedResult(h, &ov, &mut n, 1);
                    }
                    Err(io::Error::new(io::ErrorKind::TimedOut, "Discord did not answer in time"))
                }
                _ => Err(e),
            }
        }
    }

    impl io::Read for Pipe {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let (p, len) = (buf.as_mut_ptr(), buf.len().min(u32::MAX as usize) as u32);
            // SAFETY: `buf` outlives the operation (`wait` returns once it is over).
            self.wait(|h, ov| unsafe { ReadFile(h, p, len, std::ptr::null_mut(), ov) })
        }
    }

    impl io::Write for Pipe {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            let (p, len) = (buf.as_ptr(), buf.len().min(u32::MAX as usize) as u32);
            // SAFETY: as `read`'s.
            self.wait(|h, ov| unsafe { WriteFile(h, p, len, std::ptr::null_mut(), ov) })
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Drop for Pipe {
        fn drop(&mut self) {
            // SAFETY: our event, closed once; no operation is left on it (`wait` sees each to its end).
            unsafe { CloseHandle(self.event) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_fit_what_discord_takes() {
        assert_eq!(field("  "), None);
        assert_eq!(field("X").as_deref(), Some("X "));
        assert_eq!(field(" Lantern ").as_deref(), Some("Lantern"));
        let long = field(&"é".repeat(300)).unwrap();
        assert_eq!(long.chars().count(), 128);
        assert!(long.ends_with('\u{2026}'));
    }

    #[test]
    fn an_activity_is_listening_with_its_times() {
        let a = Activity { details: "Lantern".into(), state: "A".into(), start_ms: 1_000, end_ms: 246_000 };
        let v = activity_json(&a);
        assert_eq!(v, json!({ "type": 2, "status_display_type": 2, "details": "Lantern", "state": "A ", "timestamps": { "start": 1_000, "end": 246_000 }, "instance": false }));
        // No artist: no second line. No length: no end.
        let v = activity_json(&Activity { state: String::new(), end_ms: 0, ..a });
        assert!(v.get("state").is_none());
        assert!(v["timestamps"].get("end").is_none());
    }

    /// A pipe of this test's own (never Discord's), whose other end reads the handshake, answers
    /// one frame and then nothing: the next read gives up instead of waiting for ever.
    #[cfg(windows)]
    #[test]
    fn a_pipe_that_stops_answering_times_out() {
        use std::time::Instant;
        use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
        use windows_sys::Win32::Storage::FileSystem::{PIPE_ACCESS_DUPLEX, ReadFile, WriteFile};
        use windows_sys::Win32::System::Pipes::{CreateNamedPipeW, PIPE_TYPE_BYTE, PIPE_WAIT};
        let name = format!(r"\\.\pipe\mp-test-discord-ipc-{}", std::process::id());
        let wide: Vec<u16> = name.encode_utf16().chain([0]).collect();
        // SAFETY: a NUL-terminated name; the handle is closed at the end.
        let server = unsafe { CreateNamedPipeW(wide.as_ptr(), PIPE_ACCESS_DUPLEX, PIPE_TYPE_BYTE | PIPE_WAIT, 1, 4096, 4096, 0, std::ptr::null()) };
        assert_ne!(server, INVALID_HANDLE_VALUE);
        let mut pipe = win::Pipe::open(&name, Duration::from_millis(300)).unwrap();
        write_frame(&mut pipe, HANDSHAKE, &json!({ "v": 1 })).unwrap();
        let mut got = [0u8; 64];
        let mut n = 0u32;
        let mut frame = std::io::Cursor::new(Vec::new());
        write_frame(&mut frame, FRAME, &json!({ "evt": "READY" })).unwrap();
        let frame = frame.into_inner();
        // SAFETY: plain synchronous calls on our own pipe, with buffers that outlive them.
        unsafe {
            assert_ne!(ReadFile(server, got.as_mut_ptr(), got.len() as u32, &mut n, std::ptr::null_mut()), 0);
            assert_ne!(WriteFile(server, frame.as_ptr(), frame.len() as u32, &mut n, std::ptr::null_mut()), 0);
        }
        assert_eq!(&got[8..15], br#"{"v":1}"#);
        assert_eq!(read_frame(&mut pipe).unwrap(), (FRAME, json!({ "evt": "READY" })));
        let t = Instant::now();
        let e = read_frame(&mut pipe).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::TimedOut);
        assert!(t.elapsed() < Duration::from_secs(5), "{:?}", t.elapsed());
        drop(pipe);
        // SAFETY: ours, closed once.
        unsafe { CloseHandle(server) };
    }

    /// The same on Unix: a socket whose other end accepts and never writes.
    #[cfg(unix)]
    #[test]
    fn a_socket_that_never_answers_times_out() {
        let path = std::env::temp_dir().join(format!("mp-test-discord-ipc-{}", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        let mut s = unix(&path, Duration::from_millis(300)).unwrap();
        let _held = listener.accept().unwrap();
        let t = std::time::Instant::now();
        assert!(read_frame(&mut s).is_err());
        assert!(t.elapsed() < Duration::from_secs(5), "{:?}", t.elapsed());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn frames_round_trip() {
        let mut buf = std::io::Cursor::new(Vec::new());
        write_frame(&mut buf, FRAME, &json!({ "cmd": "SET_ACTIVITY" })).unwrap();
        assert_eq!(&buf.get_ref()[..8], &[1, 0, 0, 0, 22, 0, 0, 0]);
        buf.set_position(0);
        assert_eq!(read_frame(&mut buf).unwrap(), (FRAME, json!({ "cmd": "SET_ACTIVITY" })));
    }
}
