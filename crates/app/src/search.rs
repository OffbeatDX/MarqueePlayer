//! Search off the UI thread. A query folds every album and track row through Rust
//! functions (tens of milliseconds on a large library), so it runs on its own thread
//! with its own read connection (WAL: it reads while the UI and the scanner use theirs).
//!
//! Each request gets a number. The thread only runs the newest request waiting for it
//! (typing fast queues several; the older ones are skipped), and the UI shows a result
//! only if it answers the newest request sent, so a slow, stale result never replaces
//! a newer one. "See all" (one section in full, `request_section`) is a request of its own
//! kind, numbered on its own: typing never skips it.

use std::cell::Cell;
use std::path::PathBuf;

use crossbeam_channel::{Sender, unbounded};
use library::SearchResults;
use library::query::SearchKind;

use crate::state;
use crate::views;

/// Results per kind (albums, artists, tracks).
const LIMIT: usize = 24;

enum Job {
    /// The search page's sections, `LIMIT` each.
    Search(u64, String),
    /// One section in full ("See all").
    Section(u64, String, SearchKind),
}

pub struct Searcher {
    tx: Sender<Job>,
    latest: Cell<u64>,
    latest_section: Cell<u64>,
}

impl Searcher {
    pub fn new(db: PathBuf) -> Self {
        let (tx, rx) = unbounded::<Job>();
        let spawned = std::thread::Builder::new().name("mp-search".into()).spawn(move || {
            let mut conn = None;
            while let Ok(first) = rx.recv() {
                // Only the newest of each kind of what is waiting.
                let (mut search, mut section) = (None, None);
                let mut keep = |job| match job {
                    Job::Search(id, q) => search = Some((id, q)),
                    Job::Section(id, q, kind) => section = Some((id, q, kind)),
                };
                keep(first);
                while let Ok(newer) = rx.try_recv() {
                    keep(newer);
                }
                if conn.is_none() {
                    match library::db::open(&db) {
                        Ok(c) => conn = Some(c),
                        Err(e) => log::error!("search: cannot open the library ({e})"),
                    }
                }
                let run = |q: &str, f: &dyn Fn(&rusqlite::Connection) -> rusqlite::Result<SearchResults>| -> SearchResults {
                    match conn.as_ref().map(f) {
                        Some(Ok(r)) => r,
                        Some(Err(e)) => {
                            log::warn!("search {q:?}: {e}");
                            SearchResults::default()
                        }
                        None => SearchResults::default(),
                    }
                };
                // The click first: typing can wait the few milliseconds.
                if let Some((id, q, kind)) = section {
                    let t = std::time::Instant::now();
                    let results = run(&q, &|c| library::query::search_section(c, &q, kind, usize::MAX));
                    log::debug!("search all {kind:?} {q:?}: {:.1} ms", t.elapsed().as_secs_f64() * 1000.0);
                    state::post(move |st| views::show_search_all(st, id, kind, q, results));
                }
                if let Some((id, q)) = search {
                    let results = run(&q, &|c| library::query::search(c, &q, LIMIT));
                    state::post(move |st| views::show_search(st, id, results));
                }
            }
        });
        if let Err(e) = spawned {
            log::error!("cannot start the search thread: {e}");
        }
        Self { tx, latest: Cell::new(0), latest_section: Cell::new(0) }
    }

    /// Ask for section `kind` of `q` in full; it arrives in `views::show_search_all`. Its
    /// number.
    pub fn request_section(&self, q: &str, kind: SearchKind) -> u64 {
        let id = self.latest_section.get() + 1;
        self.latest_section.set(id);
        let _ = self.tx.send(Job::Section(id, q.to_owned(), kind));
        id
    }

    /// Whether `id` answers the newest "See all".
    pub fn is_latest_section(&self, id: u64) -> bool {
        id == self.latest_section.get()
    }

    /// Ask for `q`; its results arrive in `views::show_search`.
    pub fn request(&self, q: &str) {
        let id = self.latest.get() + 1;
        self.latest.set(id);
        let _ = self.tx.send(Job::Search(id, q.to_owned()));
    }

    /// Whether `id` answers the newest request.
    pub fn is_latest(&self, id: u64) -> bool {
        id == self.latest.get()
    }
}
