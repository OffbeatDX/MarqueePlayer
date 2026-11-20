//! Search off the UI thread. A query folds every album and track row through Rust
//! functions (tens of milliseconds on a large library), so it runs on its own thread
//! with its own read connection (WAL: it reads while the UI and the scanner use theirs).
//!
//! Each request gets a number. The thread only runs the newest request waiting for it
//! (typing fast queues several; the older ones are skipped), and the UI shows a result
//! only if it answers the newest request sent, so a slow, stale result never replaces
//! a newer one.

use std::cell::Cell;
use std::path::PathBuf;

use crossbeam_channel::{Sender, unbounded};
use library::SearchResults;

use crate::state;
use crate::views;

/// Results per kind (albums, artists, tracks).
const LIMIT: usize = 24;

pub struct Searcher {
    tx: Sender<(u64, String)>,
    latest: Cell<u64>,
}

impl Searcher {
    pub fn new(db: PathBuf) -> Self {
        let (tx, rx) = unbounded::<(u64, String)>();
        let spawned = std::thread::Builder::new().name("mp-search".into()).spawn(move || {
            let mut conn = None;
            while let Ok(mut job) = rx.recv() {
                // Only the newest of what is waiting.
                while let Ok(newer) = rx.try_recv() {
                    job = newer;
                }
                let (id, q) = job;
                if conn.is_none() {
                    match library::db::open(&db) {
                        Ok(c) => conn = Some(c),
                        Err(e) => log::error!("search: cannot open the library ({e})"),
                    }
                }
                let results: SearchResults = match conn.as_ref().map(|c| library::query::search(c, &q, LIMIT)) {
                    Some(Ok(r)) => r,
                    Some(Err(e)) => {
                        log::warn!("search {q:?}: {e}");
                        SearchResults::default()
                    }
                    None => SearchResults::default(),
                };
                state::post(move |st| views::show_search(st, id, results));
            }
        });
        if let Err(e) = spawned {
            log::error!("cannot start the search thread: {e}");
        }
        Self { tx, latest: Cell::new(0) }
    }

    /// Ask for `q`; its results arrive in `views::show_search`.
    pub fn request(&self, q: &str) {
        let id = self.latest.get() + 1;
        self.latest.set(id);
        let _ = self.tx.send((id, q.to_owned()));
    }

    /// Whether `id` answers the newest request.
    pub fn is_latest(&self, id: u64) -> bool {
        id == self.latest.get()
    }
}
