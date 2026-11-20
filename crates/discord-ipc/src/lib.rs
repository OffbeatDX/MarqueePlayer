//! Discord Rich Presence for Marquee Player: what plays, on the user's Discord profile
//! ("Listening to"). No UI here, and no network: it talks to the Discord app running on
//! this machine over its local IPC pipe (`ipc`), from one background thread (`presence`).
//! Discord itself tells its servers.

pub mod ipc;
pub mod presence;

pub use ipc::{Activity, Pipe};
pub use presence::{Connect, Options, Presence, State};
