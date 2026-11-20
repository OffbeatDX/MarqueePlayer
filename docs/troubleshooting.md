# When something goes wrong

What the app does when the machine, the library or its own files misbehave (each case is a test,
a fuzz test or a bench runner scenario; see [The bench runner](testing.md#the-bench-runner)):

- **A panic** (`crash.rs`): the hook logs it with its thread, place and a backtrace. One that the
  library or the player catches and recovers from (a damaged file in a scan, a decoder failing on one
  track: `library::guard::catching`, `player_core::guard::catching`) is only logged. Any other is
  fatal and said: a message box says what happened, that the library and settings are kept, where the
  log is and how to report it (Settings > About > Copy diagnostics), and the app closes. From another
  thread it closes the normal way, saving the queue and settings. On the UI thread the toolkit catches
  a panic in a window callback and would run on half-dead: the hook ends the
  event loop, the panic comes out into `main`, which shows the message and ends the process at once
  (`crash::die`: the normal exit would run thread-local destructors over the half-unwound state, and
  one of them panics); a watchdog does the same if that has not happened within 5 s. Never a silent
  disappearance, never a frozen window. The runner's `crash` scenario panics on purpose both ways.
- **The log** starts afresh at each start and keeps the previous run's as
  `marquee-player.previous.log`, so the log of a run that crashed survives the restart. **Copy
  diagnostics** (Settings > About, "Report a problem"; `about::diagnostics`) puts the version line,
  the sound device and format, the library's size and schema, the settings that change what plays, and
  the end of both logs (from a little before a crash's report) on the clipboard, with the user's home
  folder and name taken out.
- **A start that cannot go on** says why and what to do: no window (the graphics driver), a data
  folder it cannot write to (every edition now probes it, `edition::check_writable`), and the library
  database (`main::open_database`, `library::db::problem`): damaged (asks to start with a new library,
  keeping the damaged file as `library.damaged-<time>.db`, `db::set_aside`), from a newer build, held
  by another program, read-only, a full disk. A database damaged inside but whose header reads opens;
  SQLite's `quick_check` then runs on a thread of its own at every start (`check_database_soon`; tens
  of milliseconds for a few thousand tracks), and if it finds damage the app records it beside the
  file (`library.db.damaged`), says so and closes, so the next start asks as above.
- **A migration interrupted** at any moment loses nothing: each step is a transaction, and the copy
  kept before migrating is written under a temporary name and renamed when whole (a copy cut short is
  never taken for the backup). `db::tests::a_migration_killed_at_any_moment_loses_nothing` kills a
  process migrating a 30,000-track schema-3 library at 16 moments and checks every row and the backup.
- **Its own files damaged**: `settings.json` is read setting by setting when it does not read as a
  whole (one bad value costs that setting, not the music folders), the old file kept as
  `settings.json.bad` and the reset settings named in the pill; it is written to a temporary file,
  flushed and renamed, and a save that fails (read-only, full) is said once. `lastfm.json`,
  `scrobbles.json` and `navidrome.json` the same way (`.bad` copies, durable writes; a damaged
  `navidrome.json` starts signed out). `theme.toml` that does not parse: the
  built-in theme, the file left alone. A damaged tag-edit record is dropped; one whose drive is not
  connected is kept for a later start (`edit::atomic::recover`).
- **A change the library cannot take** (a full disk, the database held by another program): adding
  to, renaming or deleting a playlist, a favourite: the pill says so and the page shows what the
  library has (`views::not_written`). A play counted is kept and written by a later save, with the
  time it was counted, once (`session::record_play`, `query::record_plays`).
- **Damaged audio**: every decoder's open, seek and decode run under the player's guard (a panic is
  that file's error), a file whose packets fail 400 times in a row is "damaged" rather than racing by in
  silence, and a track that ends well short of its length (the rest of its file unreadable) says
  "stopped early" in the pill. `player-core/tests/fuzz.rs` and `library/tests/fuzz.rs` run 3,960 and
  2,673 damaged files of every format (truncated, bytes changed, inserted and deleted runs, size fields
  made huge) through the decoders, the scanner's tag reader, View metadata's reader and the art cache:
  no panic, no hang, no runaway allocation; image bombs (a PNG claiming 60,000 x 60,000) are refused.
- **A server track's download failing** (`server/streaming.rs`): a connection that fails is retried from
  where it stopped, after 0.5, 1, 2, 4 and 8 s; then, or at once when retrying cannot help (the file
  gone from the server, the sign-in refused), the pill says why and the queue moves on as for a
  damaged file; two tracks in a row failing the same way (the server unreachable) stop playback
  there. A download cut by a crash is completed from where it was at the next play, never played as
  whole (see [Streaming](features/lyrics.md#streaming); `streaming/tests.rs` and `MP_BENCH=streaming` against a local mock).
- **No sound device**: the engine says so once (`Event::NoOutput`) when the device cannot be opened or
  started, and again when it can (the pill, and Settings > Output's status), retrying meanwhile; the
  track never sits "playing" in silence without a word (`tests/output_switch.rs`).
- **Quitting with unsaved tag edits** (the tray's Quit, the close button set to quit) brings the window
  back with View metadata asking about them, and does not quit; during a save it waits for the save
  (`main::request_quit`, `metadata::view::blocks_quit`). Restart Manager and Windows shutting down still
  close at once (they cannot wait for a person).
- **A list changing under its open menu** (a smart playlist working itself out again, the queue moving
  on): a track or queue menu remembers its item, not only its row, and acts on the item wherever it is
  now, or says it is no longer there (`menus::reanchored`).
