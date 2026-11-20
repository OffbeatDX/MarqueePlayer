//! Settings > Connections: the services told what plays. `scrobble` decides when a play
//! counts and what the scrobbling services hear of it (one decision for Last.fm and
//! ListenBrainz) and is the Last.fm section; `listens` the ListenBrainz section and a track's
//! listen; `presence` the Discord section and what Discord is told plays.

pub(crate) mod listens;
pub(crate) mod presence;
pub(crate) mod scrobble;
