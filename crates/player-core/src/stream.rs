//! What the engine decodes a track from: a [`Source`] it reads itself (a file on disk, or a
//! growing file already complete), or a file still being written ([`Feed`]), decoded on a
//! thread of its own (`mp-feed`).
//!
//! Why a thread: a read of a growing file waits for its bytes, and `symphonia` cannot be
//! stopped in the middle of a packet and resumed (its demuxer and decoder state would be torn).
//! So the waiting happens on `mp-feed`, never on the engine's decode thread: that one only ever
//! takes decoded audio already queued here ([`Stream::next_chunk`] says `Dry` when there is none
//! yet) and stays free for commands. `mp-feed` decodes ahead into a queue of about a second and
//! a half; the chunks are exactly the ones `Source::next_chunk` gives, so what plays is sample
//! for sample what the same file plays from disk.
//!
//! A seek goes to `mp-feed` as a request. If its reader is waiting for bytes there, the wait is
//! interrupted, the file reopened (the cheap part: its start and any end the probe read are on
//! disk) and positioned at the target, which then waits for its own bytes. Dropping the stream
//! cancels the reader and joins the thread, so no thread is left waiting on a file.
//!
//! Which bytes a seek reads is the demuxer's business: MP4 and WAV go straight to the target's
//! offset, but FLAC and Ogg (Opus, Vorbis) bisect over the whole file, so even a seek back into
//! what has played may first read around the middle of the file and wait there. Each such wait
//! is the feed's `wanted` offset: a writer that can fetch ranges serves it in a few small reads;
//! one that only fills from the start makes the seek wait until it gets there.
//!
//! The local route is untouched: `Stream::Local` calls the `Source` directly, with no lock or
//! queue in between.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crossbeam_channel::Sender;

use crate::engine::StreamInfo;
use crate::feed::{Canceller, Feed};
use crate::replaygain::GainTags;
use crate::source::{Source, SourceError};

/// What a pull from the stream gave.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Pull {
    /// Frames were appended (or a packet was skipped): pull again.
    More,
    /// The end of the track.
    End,
    /// Nothing decoded yet: its bytes are not on disk. Pull again later.
    Dry,
}

pub(crate) enum Stream {
    Local(Source),
    Feed(FeedSource),
}

impl Stream {
    pub fn sample_rate(&self) -> u32 {
        match self {
            Stream::Local(s) => s.sample_rate,
            Stream::Feed(f) => f.info.sample_rate,
        }
    }

    /// Content frames in the track, when the container says.
    pub fn total_frames(&self) -> Option<u64> {
        match self {
            Stream::Local(s) => s.total_frames,
            Stream::Feed(f) => f.info.total_frames,
        }
    }

    pub fn gain_tags(&self) -> &GainTags {
        match self {
            Stream::Local(s) => &s.gain_tags,
            Stream::Feed(f) => &f.info.gain_tags,
        }
    }

    /// Content frame of the next frame `next_chunk` gives.
    pub fn position(&self) -> u64 {
        match self {
            Stream::Local(s) => s.position,
            Stream::Feed(f) => f.position,
        }
    }

    pub fn stream_info(&self) -> StreamInfo {
        match self {
            Stream::Local(s) => Info::of(s).stream_info(),
            Stream::Feed(f) => f.info.stream_info(),
        }
    }

    /// `Source::next_chunk`, or `Dry` for a growing file whose next frames are not decoded yet.
    #[inline]
    pub fn next_chunk(&mut self, out: &mut Vec<f32>) -> Result<Pull, SourceError> {
        match self {
            Stream::Local(s) => s.next_chunk(out).map(|more| if more { Pull::More } else { Pull::End }),
            Stream::Feed(f) => f.next_chunk(out),
        }
    }

    /// Accurate, as `Source::seek`. A growing file is positioned on its own thread: frames from
    /// the target come once they are on disk.
    pub fn seek(&mut self, frame: u64) -> Result<(), SourceError> {
        match self {
            Stream::Local(s) => s.seek(frame),
            Stream::Feed(f) => {
                f.seek(frame);
                Ok(())
            }
        }
    }

    /// `frames` can be pulled without running dry (or the end or an error comes first). Always
    /// for a local file. For a growing one, at most the queue's length is asked for.
    pub fn ready(&self, frames: usize) -> bool {
        match self {
            Stream::Local(_) => true,
            Stream::Feed(f) => f.ready(frames),
        }
    }
}

/// What the opened file says about itself.
#[derive(Clone, Debug)]
pub(crate) struct Info {
    pub sample_rate: u32,
    pub total_frames: Option<u64>,
    pub gain_tags: GainTags,
    codec_name: Option<&'static str>,
    bits_per_sample: Option<u32>,
    channels: Option<u32>,
}

impl Info {
    fn of(s: &Source) -> Self {
        Info {
            sample_rate: s.sample_rate,
            total_frames: s.total_frames,
            gain_tags: s.gain_tags.clone(),
            codec_name: s.codec_name,
            bits_per_sample: s.bits_per_sample,
            channels: s.channels,
        }
    }

    fn stream_info(&self) -> StreamInfo {
        StreamInfo {
            codec: self.codec_name,
            sample_rate: self.sample_rate,
            bits_per_sample: self.bits_per_sample,
            channels: self.channels,
            gain_tags: self.gain_tags.clone(),
        }
    }
}

/// Decoded on `mp-feed`, on its way to the engine.
enum Item {
    /// Interleaved stereo, and the content frame after its last.
    Audio { samples: Vec<f32>, end: u64 },
    End,
    Error(String),
}

struct PipeState {
    /// Set once by `mp-feed` when the file is open (or could not be).
    opened: Option<Result<Info, String>>,
    items: VecDeque<Item>,
    /// Frames in `items`.
    frames: usize,
    /// `mp-feed` decodes no further ahead than this many frames.
    capacity: usize,
    /// Bumped by every seek: what `mp-feed` decoded before it is dropped.
    epoch: u64,
    seek: Option<u64>,
    cancelled: bool,
}

struct Pipe {
    state: Mutex<PipeState>,
    /// `mp-feed` waits on it for room, a seek or the end.
    changed: Condvar,
}

impl Pipe {
    fn lock(&self) -> MutexGuard<'_, PipeState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// `mp-feed` and the handle that stops it.
struct Worker {
    pipe: Arc<Pipe>,
    cancel: Canceller,
    thread: Option<JoinHandle<()>>,
}

impl Drop for Worker {
    /// Cancel any read waiting and join the thread: it returns within a packet's decode.
    fn drop(&mut self) {
        self.pipe.lock().cancelled = true;
        self.pipe.changed.notify_all();
        self.cancel.cancel();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// A growing file being opened on `mp-feed` (the bytes the probe reads may not be on disk yet).
pub(crate) struct Opening {
    worker: Option<Worker>,
}

impl Opening {
    /// Start opening `path`, written through `feed`. `wake` is signalled whenever there is
    /// something new to take (the open done, decoded audio, the end).
    pub fn spawn(path: &Path, feed: &Feed, wake: Sender<()>) -> Self {
        let pipe = Arc::new(Pipe {
            state: Mutex::new(PipeState { opened: None, items: VecDeque::new(), frames: 0, capacity: 0, epoch: 0, seek: None, cancelled: false }),
            changed: Condvar::new(),
        });
        let cancel = feed.canceller();
        let thread = {
            let (pipe, cancel, feed, path) = (pipe.clone(), cancel.clone(), feed.clone(), path.to_path_buf());
            thread::Builder::new().name("mp-feed".into()).spawn(move || run(path, feed, pipe, cancel, wake)).expect("spawn feed thread")
        };
        Opening { worker: Some(Worker { pipe, cancel, thread: Some(thread) }) }
    }

    /// The opened stream, or why it could not be opened, once `mp-feed` knows. Taken once.
    pub fn poll(&mut self) -> Option<Result<FeedSource, String>> {
        let opened = self.worker.as_ref()?.pipe.lock().opened.clone()?;
        let worker = self.worker.take()?;
        Some(opened.map(|info| FeedSource { info, position: 0, worker }))
    }

    /// `poll`, waiting up to `timeout`.
    pub fn wait(&mut self, timeout: Duration) -> Option<Result<FeedSource, String>> {
        let deadline = Instant::now() + timeout;
        {
            let pipe = self.worker.as_ref()?.pipe.clone();
            let mut st = pipe.lock();
            while st.opened.is_none() {
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    return None;
                }
                st = pipe.changed.wait_timeout(st, left).unwrap_or_else(|e| e.into_inner()).0;
            }
        }
        self.poll()
    }
}

/// A growing file, open, decoding on `mp-feed`.
pub(crate) struct FeedSource {
    info: Info,
    position: u64,
    worker: Worker,
}

impl FeedSource {
    fn next_chunk(&mut self, out: &mut Vec<f32>) -> Result<Pull, SourceError> {
        let pipe = &self.worker.pipe;
        let mut st = pipe.lock();
        match st.items.front() {
            None => Ok(Pull::Dry),
            // Both stay: a pull after the end gives the end again.
            Some(Item::End) => Ok(Pull::End),
            Some(Item::Error(m)) => Err(SourceError(m.clone())),
            Some(Item::Audio { .. }) => {
                let Some(Item::Audio { samples, end }) = st.items.pop_front() else { unreachable!() };
                st.frames -= samples.len() / 2;
                drop(st);
                pipe.changed.notify_all();
                out.extend_from_slice(&samples);
                self.position = end;
                Ok(Pull::More)
            }
        }
    }

    fn ready(&self, frames: usize) -> bool {
        let st = self.worker.pipe.lock();
        st.frames >= frames.min(st.capacity) || matches!(st.items.back(), Some(Item::End | Item::Error(_)))
    }

    fn seek(&mut self, frame: u64) {
        let pipe = &self.worker.pipe;
        {
            let mut st = pipe.lock();
            st.items.clear();
            st.frames = 0;
            st.epoch += 1;
            st.seek = Some(frame);
            // Under the lock, so `mp-feed` cannot take the seek before the flag is up and then
            // find it up, stale, at a later wait.
            self.worker.cancel.interrupt();
        }
        pipe.changed.notify_all();
        self.worker.cancel.wake();
        self.position = frame;
    }
}

/// `mp-feed`: open the file, then decode ahead into the pipe, serving seeks, until cancelled.
fn run(path: PathBuf, feed: Feed, pipe: Arc<Pipe>, cancel: Canceller, wake: Sender<()>) {
    let signal = || {
        let _ = wake.try_send(());
    };
    // The writer's message says what went wrong better than the decoder's error from it.
    let why = |e: SourceError| feed.failure().unwrap_or(e.0);
    if !feed.wait_length(&cancel) {
        return;
    }
    let open = || Source::open_feed(&path, &feed, &cancel);
    let src = match open() {
        Ok(s) => s,
        Err(e) => {
            if !cancel.cancelled() {
                pipe.lock().opened = Some(Err(why(e)));
                pipe.changed.notify_all();
                signal();
            }
            return;
        }
    };
    {
        let mut st = pipe.lock();
        st.capacity = (src.sample_rate as usize * 3 / 2).max(32_768);
        st.opened = Some(Ok(Info::of(&src)));
    }
    pipe.changed.notify_all();
    signal();

    // `src` is `None` after a read was interrupted (its state is unknown): it is reopened.
    let mut src = Some(src);
    let mut epoch = 0;
    // The end (or an error) is queued for this epoch: nothing more until a seek.
    let mut ended = false;
    // A reposition due: a seek's target, or where an interrupted read was.
    let mut target: Option<u64> = None;
    loop {
        {
            let mut st = pipe.lock();
            loop {
                if st.cancelled || cancel.cancelled() {
                    return;
                }
                if let Some(f) = st.seek.take() {
                    epoch = st.epoch;
                    cancel.clear_interrupt();
                    ended = false;
                    target = Some(f);
                    break;
                }
                // The flag goes up only with a seek, under this lock: none is pending.
                cancel.clear_interrupt();
                if !ended && (target.is_some() || src.is_none() || st.frames < st.capacity) {
                    break;
                }
                st = pipe.changed.wait(st).unwrap_or_else(|e| e.into_inner());
            }
        }
        let push = |item: Item, ended: &mut bool| {
            let mut st = pipe.lock();
            if st.epoch == epoch {
                *ended |= !matches!(item, Item::Audio { .. });
                if let Item::Audio { samples, .. } = &item {
                    st.frames += samples.len() / 2;
                }
                st.items.push_back(item);
            }
            drop(st);
            signal();
        };
        let Some(s) = src.as_mut() else {
            match open() {
                Ok(s) => src = Some(s),
                Err(e) if !cancel.cancelled() && !cancel.interrupted() => push(Item::Error(why(e)), &mut ended),
                Err(_) => {}
            }
            continue;
        };
        if let Some(f) = target {
            match s.seek(f) {
                Ok(()) => target = None,
                Err(_) if cancel.cancelled() => return,
                // Another seek came while this one waited for its bytes.
                Err(_) if cancel.interrupted() => src = None,
                Err(e) => {
                    target = None;
                    if feed.failure().is_some() {
                        push(Item::Error(why(e)), &mut ended);
                    } else {
                        // As for a file on disk: the decoding goes on from wherever it is.
                        log::warn!("seek failed: {e}");
                    }
                }
            }
            continue;
        }
        let mut buf = Vec::new();
        match s.next_chunk(&mut buf) {
            Ok(true) if buf.is_empty() => {}
            Ok(true) => push(Item::Audio { samples: buf, end: s.position }, &mut ended),
            Ok(false) => push(Item::End, &mut ended),
            Err(_) if cancel.cancelled() => return,
            Err(_) if cancel.interrupted() => {
                // A seek is waiting to be taken; should it not be, carry on from here.
                target = Some(s.position);
                src = None;
            }
            Err(e) => push(Item::Error(why(e)), &mut ended),
        }
    }
}
