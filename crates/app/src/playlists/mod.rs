//! Playlists beyond the library's own tables. `smart` is smart playlists in the app (the
//! thread that works them out, their page and the editor) and `smartedit` the editor's draft;
//! `file` M3U import and export; `newplaylist` the name prompt of "New playlist…";
//! `nameclash` the prompt for a playlist name already in use.

pub(crate) mod file;
pub(crate) mod nameclash;
pub(crate) mod newplaylist;
pub(crate) mod smart;
pub(crate) mod smartedit;
