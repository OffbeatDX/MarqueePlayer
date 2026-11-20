//! ListenBrainz listens for Marquee Player: the API (`api`), the submitter thread with its
//! durable queue and account file (`submitter`), over the shared HTTP client. No UI here.
//! Feature `mock`: a ListenBrainz stand-in on 127.0.0.1 for tests and the bench.
//!
//! When a play counts is the app's rule (the same as Last.fm's, which ListenBrainz documents
//! too: half the track or four minutes, whichever is lower); this crate sends what it is given.

pub mod api;
#[cfg(feature = "mock")]
pub mod mock;
pub mod submitter;

pub use api::{API_ROOT, ApiError, Client, Ids, Listen, ListenType, TOKEN_PAGE};
pub use submitter::{Account, Options, Sealer, State, Status, Submitter};
