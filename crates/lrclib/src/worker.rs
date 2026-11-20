//! The lookup thread. Requests come with a generation (the app's track change counter);
//! only the newest one is answered. A lookup waits `delay` before it may use the
//! network, and a newer request in that time replaces it, so skipping through tracks
//! sends nothing. One request at a time; a 429 or 503 pauses all lookups for its
//! `Retry-After`. The thread ends when the `Worker` is dropped (or with the process: no
//! one ever waits for it, so a request in flight never holds up an exit).

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crossbeam_channel::{RecvTimeoutError, Sender, unbounded};

use crate::{Answer, Cache, Client, Error, TrackInfo};

/// What a lookup came to.
#[derive(Clone, Debug, PartialEq)]
pub enum Delivery {
    Answer(Answer),
    /// The network could not be reached (nothing cached; try again later).
    Offline(String),
    /// The service asked to slow down, or answered with an error (nothing cached).
    Failed(String),
    /// Online lookups are off: nothing was sent.
    Disabled,
}

struct Job {
    generation: u64,
    track: TrackInfo,
    not_before: Instant,
    /// The user asked again: forget a "not found" or a rejection first.
    retry: bool,
}

enum Msg {
    Lookup(Job),
    Reject(TrackInfo),
}

pub struct Worker {
    tx: Sender<Msg>,
    current: Arc<AtomicU64>,
}

/// Called on the worker thread with each answer: `(generation, what came of it)`.
pub type Deliver = Box<dyn Fn(u64, Delivery) + Send>;

impl Worker {
    /// Start the thread. `deliver` is called on it with each answer for the newest
    /// generation; `enabled` is read before every request (off: nothing is sent).
    pub fn spawn(client: Client, cache: Cache, enabled: Arc<AtomicBool>, deliver: Deliver) -> Self {
        let (tx, rx) = unbounded::<Msg>();
        let current = Arc::new(AtomicU64::new(0));
        let cur = current.clone();
        let spawned = std::thread::Builder::new().name("mp-lrclib".into()).spawn(move || {
            let mut pending: VecDeque<Msg> = VecDeque::new();
            let mut backoff: Option<Instant> = None;
            loop {
                let msg = match pending.pop_front() {
                    Some(m) => m,
                    None => match rx.recv() {
                        Ok(m) => m,
                        Err(_) => return,
                    },
                };
                let job = match msg {
                    Msg::Reject(t) => {
                        cache.reject(&t);
                        continue;
                    }
                    Msg::Lookup(j) => j,
                };
                if cur.load(Ordering::Acquire) != job.generation {
                    continue;
                }
                // A cached answer needs no wait (and sends nothing).
                if !job.retry
                    && let Some(a) = cache.get(&job.track).answer
                {
                    deliver(job.generation, Delivery::Answer(a));
                    continue;
                }
                // Wait out the delay; a newer request replaces this one, while rejections
                // still take effect in order.
                let mut superseded = false;
                while Instant::now() < job.not_before {
                    match rx.recv_deadline(job.not_before) {
                        Ok(Msg::Lookup(newer)) => {
                            pending.push_back(Msg::Lookup(newer));
                            superseded = true;
                            break;
                        }
                        Ok(Msg::Reject(t)) => cache.reject(&t),
                        Err(RecvTimeoutError::Timeout) => break,
                        Err(RecvTimeoutError::Disconnected) => return,
                    }
                }
                if superseded {
                    continue;
                }
                // Anything queued behind it: a newer lookup supersedes this one; rejections stay
                // in FIFO order so a reject sent after this lookup still runs after its put.
                for m in rx.try_iter() {
                    if matches!(m, Msg::Lookup(_)) {
                        superseded = true;
                    }
                    pending.push_back(m);
                }
                if superseded {
                    continue;
                }
                let g = job.generation;
                if cur.load(Ordering::Acquire) != g {
                    continue;
                }
                let t = &job.track;
                if job.retry {
                    cache.retry(t);
                }
                let cached = cache.get(t);
                if let Some(a) = cached.answer {
                    deliver(g, Delivery::Answer(a));
                    continue;
                }
                if !enabled.load(Ordering::Acquire) {
                    deliver(g, Delivery::Disabled);
                    continue;
                }
                if backoff.is_some_and(|b| Instant::now() < b) {
                    deliver(g, Delivery::Failed("the lyrics service asked to wait".into()));
                    continue;
                }
                let result = client.lookup(t, &cached.rejected);
                let fresh = cur.load(Ordering::Acquire) == g;
                match result {
                    Ok(a) => {
                        // Cached whether or not the track is still playing.
                        cache.put(t, &a);
                        if fresh {
                            deliver(g, Delivery::Answer(a));
                        }
                    }
                    Err(Error::Busy(after)) => {
                        let secs = after.unwrap_or(30).clamp(1, 3600);
                        // Up to 30% more, as the service asks, so clients do not all return at once.
                        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.subsec_nanos());
                        let jitter = f64::from(nanos % 300) / 1000.0;
                        backoff = Some(Instant::now() + Duration::from_secs_f64(secs as f64 * (1.0 + jitter)));
                        log::info!("lyrics lookup: service busy; waiting {secs} s");
                        if fresh {
                            deliver(g, Delivery::Failed("the lyrics service is busy".into()));
                        }
                    }
                    Err(Error::Network(m)) => {
                        log::info!("lyrics lookup: offline ({m})");
                        if fresh {
                            deliver(g, Delivery::Offline(m));
                        }
                    }
                    Err(Error::Server(m)) => {
                        log::warn!("lyrics lookup: {m}");
                        if fresh {
                            deliver(g, Delivery::Failed(m));
                        }
                    }
                }
            }
        });
        if let Err(e) = spawned {
            log::error!("cannot start the lyrics lookup thread: {e}");
        }
        Self { tx, current }
    }

    /// Look up `track` for this generation once `delay` has passed (a cached answer may
    /// come at once). Any older request is dropped.
    pub fn lookup(&self, generation: u64, track: TrackInfo, delay: Duration) {
        self.send(generation, track, delay, false);
    }

    /// Ask again now (the user's "Try again"): a "not found" or rejection is forgotten
    /// first; rejected records stay refused.
    pub fn retry(&self, generation: u64, track: TrackInfo) {
        self.send(generation, track, Duration::ZERO, true);
    }

    /// Mark `track`'s cached lyrics as wrong on the worker thread, serialized with any
    /// in-flight lookup or later retry so a reject after a put always wins.
    pub fn reject(&self, track: TrackInfo) {
        let _ = self.tx.send(Msg::Reject(track));
    }

    /// The track changed to one that is not looked up: drop any request and any answer
    /// still on its way.
    pub fn cancel(&self, generation: u64) {
        self.current.store(generation, Ordering::Release);
    }

    fn send(&self, generation: u64, track: TrackInfo, delay: Duration, retry: bool) {
        self.current.store(generation, Ordering::Release);
        let _ = self.tx.send(Msg::Lookup(Job { generation, track, not_before: Instant::now() + delay, retry }));
    }
}
