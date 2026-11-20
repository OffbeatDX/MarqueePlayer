//! Last.fm scrobbling for Marquee Player: the API (`api`), the scrobbler thread with its
//! durable queue and credentials file (`scrobbler`), over the shared HTTP client (`http-client`), signed with
//! MD5 (the `md5` crate, re-exported here). No UI here. When a play is a scrobble is the app's
//! rule (`scrobble::counts`), shared with ListenBrainz.

pub mod api;
pub use md5;
pub mod scrobbler;

pub use api::{ApiError, Client, Outcome, Scrobble};
pub use scrobbler::{Credentials, Options, Scrobbler, Sealer, State, Status};
