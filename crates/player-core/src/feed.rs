//! A file that is still being written: a [`Feed`] is shared by whoever writes it (a
//! downloader, which this crate knows nothing about) and the engine, which plays the file
//! from disk while it grows. Nothing here does any networking.
//!
//! The writer says how long the file will be ([`Feed::set_length`], usually before the first
//! byte, from a Content-Length), which bytes are on disk ([`Feed::advance`] for the prefix
//! from 0; [`Feed::fill`] for a stretch further on, for a writer that fetches ranges out of
//! order), and how it ended ([`Feed::complete`] or [`Feed::fail`]). A [`FeedReader`] is a
//! `symphonia` `MediaSource` over the same file: a read whose first byte is not on disk yet
//! blocks until it is, returns end of file at the true end, fails with the writer's message
//! once the writer has failed, and returns at once when cancelled from another thread
//! ([`Canceller`]). A read is served as far as the bytes on disk go (a short read), so a
//! reader only ever waits for the very next byte it needs.
//!
//! While a reader waits, the offset it waits for is the feed's [`Feed::wanted`]: a writer
//! that can fetch ranges can go there first. That matters at open for two kinds of file: an
//! Ogg file (Opus, Vorbis), whose last page gives its length, and an MP4 whose `moov` box is
//! after its audio (over 128 KB of it, which `symphonia` seeks past). Both wait for the end of
//! the file before anything plays: with a writer that only fills from the start, for the whole
//! download; with one that fetches the stretch `wanted` asks for ([`Feed::fill`]), within
//! moments. FLAC, MP3, WAV and an MP4 with `moov` first open from their start (the probe's look
//! for trailing tags finds no reader for them in this build). Seeks are the other case
//! (`stream.rs`): FLAC and Ogg bisect over the whole file.
//!
//! The engine never blocks on a feed on its own threads: the decoding of a growing file runs on
//! a thread of its own (`stream.rs`), which is where a reader waits.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

use symphonia::core::io::MediaSource;

/// What the writer has said about the file's length.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Length {
    /// Nothing yet.
    Undecided,
    /// It will be this long.
    Known(u64),
    /// It will not be known before the file is complete (no Content-Length).
    Unknown,
}

struct State {
    length: Length,
    /// The stretches on disk, sorted, disjoint and not touching.
    filled: Vec<Range<u64>>,
    complete: bool,
    failed: Option<String>,
    /// Readers waiting, and the offset each waits for.
    waiting: Vec<(u64, u64)>,
    readers: usize,
}

impl State {
    /// Where the bytes on disk from `pos` on end, if `pos` is on disk.
    fn on_disk_to(&self, pos: u64) -> Option<u64> {
        self.filled.iter().find(|r| r.start <= pos && pos < r.end).map(|r| r.end)
    }

    /// The file's length, once it is known.
    fn len(&self) -> Option<u64> {
        match self.length {
            Length::Known(n) => Some(n),
            _ if self.complete => Some(self.filled.last().map_or(0, |r| r.end)),
            _ => None,
        }
    }

    fn add(&mut self, r: Range<u64>) {
        if r.start >= r.end {
            return;
        }
        let (mut start, mut end) = (r.start, r.end);
        // Merge with every stretch it overlaps or touches.
        self.filled.retain(|f| {
            if f.end < start || f.start > end {
                return true;
            }
            start = start.min(f.start);
            end = end.max(f.end);
            false
        });
        let at = self.filled.partition_point(|f| f.start < start);
        self.filled.insert(at, start..end);
    }
}

struct Inner {
    state: Mutex<State>,
    changed: Condvar,
}

impl Inner {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// One file being written while it is played: the writer's and the readers' shared handle.
/// Cheap to clone (all clones are the same feed).
#[derive(Clone)]
pub struct Feed(Arc<Inner>);

impl Default for Feed {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for Feed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = self.0.lock();
        f.debug_struct("Feed")
            .field("length", &s.length)
            .field("filled", &s.filled)
            .field("complete", &s.complete)
            .field("failed", &s.failed)
            .finish()
    }
}

/// Two handles are equal when they are the same feed.
impl PartialEq for Feed {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

static READER_IDS: AtomicU64 = AtomicU64::new(1);

impl Feed {
    pub fn new() -> Self {
        Feed(Arc::new(Inner {
            state: Mutex::new(State { length: Length::Undecided, filled: Vec::new(), complete: false, failed: None, waiting: Vec::new(), readers: 0 }),
            changed: Condvar::new(),
        }))
    }

    fn update(&self, f: impl FnOnce(&mut State)) {
        let mut s = self.0.lock();
        f(&mut s);
        drop(s);
        self.0.changed.notify_all();
    }

    // The writer's side.

    /// The file will be `Some(len)` bytes long (a Content-Length), or `None`: its length will
    /// not be known before it is complete. Until this is said, the engine does not open the
    /// file (whether it can be read to its end decides how it is opened). Without a length,
    /// nothing can read the end of the file before it is all there, so the file is not seekable
    /// until then (no trailing tags at open, no Ogg duration, no seeking).
    pub fn set_length(&self, len: Option<u64>) {
        self.update(|s| s.length = len.map_or(Length::Unknown, Length::Known));
    }

    /// The bytes `0..to` are on disk (written and flushed to the file, so another handle on it
    /// reads them).
    pub fn advance(&self, to: u64) {
        self.fill(0..to);
    }

    /// The bytes in `range` are on disk: for a writer that fetches a stretch further on first
    /// (the end of the file for the probe, a seek's target) and fills the rest later. Stretches
    /// may come in any order and overlap.
    pub fn fill(&self, range: Range<u64>) {
        self.update(|s| s.add(range));
    }

    /// The file is complete on disk. Without a known length, its length is where the bytes on
    /// disk end.
    pub fn complete(&self) {
        self.update(|s| {
            let end = match s.length {
                Length::Known(n) => n,
                _ => s.filled.last().map_or(0, |r| r.end),
            };
            s.add(0..end);
            s.complete = true;
        });
    }

    /// The writing failed and nothing more will come. A reader still gets every byte already on
    /// disk; the first read that needs one that is not fails with `message`, and the engine
    /// reports it as the track's error.
    pub fn fail(&self, message: impl Into<String>) {
        let message = message.into();
        self.update(|s| s.failed = Some(message));
    }

    /// The offset a reader is waiting for, which is not on disk: the lowest if several wait.
    /// `None` while no reader waits. A writer that can fetch ranges goes there first.
    pub fn wanted(&self) -> Option<u64> {
        let s = self.0.lock();
        s.waiting.iter().map(|&(_, pos)| pos).filter(|&p| s.on_disk_to(p).is_none()).min()
    }

    /// Readers open on the file now (the engine's, while it plays or has the track queued).
    /// It drops to 0 once the engine lets go of the track (stopped, skipped, the next track
    /// changed), every waiting read having returned.
    pub fn readers(&self) -> usize {
        self.0.lock().readers
    }

    // Both sides.

    /// The whole file is on disk (`complete`, or every byte of a known length filled).
    pub fn is_complete(&self) -> bool {
        let s = self.0.lock();
        s.complete || matches!(s.length, Length::Known(n) if s.on_disk_to(0).is_some_and(|e| e >= n) || n == 0)
    }

    /// The writer's message, once it has failed.
    pub fn failure(&self) -> Option<String> {
        self.0.lock().failed.clone()
    }

    /// The length is decided: known, or said not to be known before the end, or complete, or
    /// failed. `cancel` stops the wait (false then).
    pub(crate) fn wait_length(&self, cancel: &Canceller) -> bool {
        let mut s = self.0.lock();
        loop {
            if cancel.cancelled() {
                return false;
            }
            if s.length != Length::Undecided || s.complete || s.failed.is_some() {
                return true;
            }
            s = self.0.changed.wait(s).unwrap_or_else(|e| e.into_inner());
        }
    }

    // The readers' side.

    /// A reader over the file at `path`, which `cancel` can stop from another thread. The file
    /// is opened at the first read that has bytes to read.
    pub fn reader(&self, path: &Path, cancel: &Canceller) -> FeedReader {
        self.update(|s| s.readers += 1);
        FeedReader { feed: self.clone(), path: path.to_path_buf(), file: None, pos: 0, file_pos: None, cancel: cancel.clone(), id: READER_IDS.fetch_add(1, Ordering::Relaxed) }
    }

    /// A canceller for readers of this feed.
    pub fn canceller(&self) -> Canceller {
        Canceller { feed: self.clone(), flags: Arc::new(Flags::default()) }
    }

    fn wake(&self) {
        let _s = self.0.lock();
        self.0.changed.notify_all();
    }
}

#[derive(Default)]
struct Flags {
    cancelled: AtomicBool,
    interrupted: AtomicBool,
}

/// Stops the readers made with it: a read waiting returns at once with an error, and so does
/// every later read that would wait. Cheap to clone.
#[derive(Clone)]
pub struct Canceller {
    feed: Feed,
    flags: Arc<Flags>,
}

impl Canceller {
    /// For good: the readers are being dropped.
    pub fn cancel(&self) {
        self.flags.cancelled.store(true, Ordering::Release);
        self.feed.wake();
    }

    pub fn cancelled(&self) -> bool {
        self.flags.cancelled.load(Ordering::Acquire)
    }

    /// Only the wait under way, and any before `clear_interrupt`: the engine's seek, which
    /// repositions anyway. A wait under way returns once `wake` is called.
    pub(crate) fn interrupt(&self) {
        self.flags.interrupted.store(true, Ordering::Release);
    }

    pub(crate) fn wake(&self) {
        self.feed.wake();
    }

    pub(crate) fn clear_interrupt(&self) {
        self.flags.interrupted.store(false, Ordering::Release);
    }

    pub(crate) fn interrupted(&self) -> bool {
        self.flags.interrupted.load(Ordering::Acquire)
    }

    fn stopped(&self) -> Option<&'static str> {
        if self.cancelled() {
            Some("cancelled")
        } else if self.interrupted() {
            Some("interrupted")
        } else {
            None
        }
    }
}

/// A `MediaSource` over a file being written (see the module doc).
pub struct FeedReader {
    feed: Feed,
    path: PathBuf,
    file: Option<File>,
    pos: u64,
    /// Where the file's own cursor is, when known.
    file_pos: Option<u64>,
    cancel: Canceller,
    id: u64,
}

impl Drop for FeedReader {
    fn drop(&mut self) {
        self.feed.update(|s| {
            s.readers -= 1;
            s.waiting.retain(|w| w.0 != self.id);
        });
    }
}

/// What a read at the reader's position can do now.
enum Ready {
    /// Read up to here.
    To(u64),
    Eof,
    Failed(String),
}

impl FeedReader {
    /// Wait until the byte at `pos` is on disk, or the end, or a failure; or the reader is
    /// stopped (an error).
    fn ready(&self, pos: u64) -> io::Result<Ready> {
        let inner = &self.feed.0;
        let mut s = inner.lock();
        let mut waiting = false;
        let r = loop {
            if let Some(end) = s.on_disk_to(pos) {
                break Ok(Ready::To(end));
            }
            if s.len().is_some_and(|len| pos >= len) {
                break Ok(Ready::Eof);
            }
            if let Some(m) = &s.failed {
                break Ok(Ready::Failed(m.clone()));
            }
            if let Some(why) = self.cancel.stopped() {
                break Err(io::Error::other(format!("reading the file {why}")));
            }
            if !waiting {
                waiting = true;
                s.waiting.push((self.id, pos));
                inner.changed.notify_all();
            }
            s = inner.changed.wait(s).unwrap_or_else(|e| e.into_inner());
        };
        if waiting {
            s.waiting.retain(|w| w.0 != self.id);
        }
        r
    }

    fn len_now(&self) -> Option<u64> {
        self.feed.0.lock().len()
    }
}

impl Read for FeedReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let end = match self.ready(self.pos)? {
            Ready::To(end) => end,
            Ready::Eof => return Ok(0),
            Ready::Failed(m) => return Err(io::Error::other(m)),
        };
        let n = ((end - self.pos).min(buf.len() as u64)) as usize;
        if self.file.is_none() {
            self.file = Some(File::open(&self.path)?);
            self.file_pos = Some(0);
        }
        let file = self.file.as_mut().unwrap();
        if self.file_pos != Some(self.pos) {
            file.seek(SeekFrom::Start(self.pos))?;
        }
        let got = file.read(&mut buf[..n]);
        let got = match got {
            Ok(0) => {
                self.file_pos = None;
                return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "the file is shorter than its writer said"));
            }
            Ok(g) => g,
            Err(e) => {
                self.file_pos = None;
                return Err(e);
            }
        };
        self.pos += got as u64;
        self.file_pos = Some(self.pos);
        Ok(got)
    }
}

impl Seek for FeedReader {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let pos = match to {
            SeekFrom::Start(n) => Some(n),
            SeekFrom::Current(d) => self.pos.checked_add_signed(d),
            SeekFrom::End(d) => {
                // The length, once the writer has said it (it is said before the engine opens
                // the file, or it is not known before the file is complete).
                let len = loop {
                    if let Some(len) = self.len_now() {
                        break len;
                    }
                    match self.ready(u64::MAX)? {
                        Ready::Failed(m) => return Err(io::Error::other(m)),
                        _ => continue,
                    }
                };
                len.checked_add_signed(d)
            }
        };
        self.pos = pos.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "seek before the start"))?;
        Ok(self.pos)
    }
}

impl MediaSource for FeedReader {
    /// Seekable once its length is known: every reposition then works, the reads waiting for
    /// their bytes as any other.
    fn is_seekable(&self) -> bool {
        self.len_now().is_some()
    }

    fn byte_len(&self) -> Option<u64> {
        self.len_now()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::time::{Duration, Instant};

    fn file(name: &str, data: &[u8]) -> PathBuf {
        let p = std::env::temp_dir().join(format!("mp-feed-{}-{name}", std::process::id()));
        std::fs::write(&p, data).unwrap();
        p
    }

    #[test]
    fn stretches_merge() {
        let f = Feed::new();
        f.fill(10..20);
        f.fill(30..40);
        f.fill(20..25);
        f.fill(0..5);
        assert_eq!(f.0.lock().filled, vec![0..5, 10..25, 30..40]);
        f.fill(4..31);
        assert_eq!(f.0.lock().filled, vec![0..40]);
        f.set_length(Some(40));
        assert!(f.is_complete());
    }

    #[test]
    fn reads_wait_for_their_bytes_and_end_at_the_length() {
        let data: Vec<u8> = (0..=255u8).cycle().take(10_000).collect();
        let p = file("wait", &data);
        let f = Feed::new();
        f.set_length(Some(10_000));
        let c = f.canceller();
        let mut r = f.reader(&p, &c);
        let w = {
            let f = f.clone();
            std::thread::spawn(move || {
                for to in (1000..=10_000).step_by(1000) {
                    std::thread::sleep(Duration::from_millis(5));
                    f.advance(to);
                }
            })
        };
        let mut got = Vec::new();
        r.read_to_end(&mut got).unwrap();
        assert_eq!(got, data);
        w.join().unwrap();
        // A stretch further on, then a seek back.
        r.seek(SeekFrom::Start(9_990)).unwrap();
        let mut b = [0u8; 100];
        assert_eq!(r.read(&mut b).unwrap(), 10);
        assert_eq!(r.read(&mut b).unwrap(), 0);
        drop(r);
        assert_eq!(f.readers(), 0);
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn a_waiting_read_is_cancelled_and_reports_what_it_wants() {
        let p = file("cancel", &[7u8; 100]);
        let f = Feed::new();
        f.set_length(Some(1000));
        f.advance(100);
        let c = f.canceller();
        let mut r = f.reader(&p, &c);
        r.seek(SeekFrom::Start(500)).unwrap();
        let t = std::thread::spawn(move || {
            let mut b = [0u8; 10];
            r.read(&mut b).map(|_| ())
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        while f.wanted() != Some(500) {
            assert!(Instant::now() < deadline, "the reader waits at 500");
            std::thread::sleep(Duration::from_millis(1));
        }
        let at = Instant::now();
        c.cancel();
        assert!(t.join().unwrap().is_err());
        assert!(at.elapsed() < Duration::from_millis(500));
        assert_eq!(f.wanted(), None);
        assert_eq!(f.readers(), 0);
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn bytes_on_disk_are_read_after_a_failure_then_the_failure() {
        let p = std::env::temp_dir().join(format!("mp-feed-{}-fail", std::process::id()));
        let mut w = File::create(&p).unwrap();
        w.write_all(&[1u8; 64]).unwrap();
        let f = Feed::new();
        f.set_length(Some(1000));
        f.advance(64);
        f.fail("the server went away");
        let c = f.canceller();
        let mut r = f.reader(&p, &c);
        let mut b = [0u8; 100];
        assert_eq!(r.read(&mut b).unwrap(), 64);
        assert_eq!(r.read(&mut b).unwrap_err().to_string(), "the server went away");
        drop(w);
        let _ = std::fs::remove_file(p);
    }
}
