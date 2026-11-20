# Marquee Player for Android: design

An Android client for one Navidrome server, carrying the desktop Marquee Player's look
(dark, artwork-led, the accent taken from the playing cover) to a phone. It plays from
the server instead of local files and puts battery life first. It has no visualizers,
and only a short list of settings.

This document holds the design decisions and their reasons, the rules every change follows
(battery, testing), and the decisions taken while building it. [The README](../README.md)
describes the app as it is: building, settings, privacy, testing.

This folder is self-contained: nothing outside `android/` is changed by Android work
(not the root README, `.gitignore`, `Cargo.toml` or any desktop crate).

## Decisions

| Topic | Decision |
| --- | --- |
| Stack | Kotlin, Jetpack Compose, Media3 (ExoPlayer + MediaLibraryService). No Rust. Desktop algorithms are ported, not shared |
| Server | One Navidrome server (developed against 0.64.2), one public HTTPS URL. Subsonic API 1.16.1 + OpenSubsonic extensions; token auth (salt + md5 token), never the plain password stored |
| Offline | Manual downloads (pin an album or playlist) + an automatic cache: the next tracks fetched whole ahead of time, recent plays kept, size-capped |
| Quality | Original on Wi-Fi, Opus (128 kbps default) on mobile data; both adjustable. Downloads have their own setting |
| Audio | Gapless; ReplayGain (track / album / off) applied as player volume; sleep timer; built-in EQ, off by default (10-band graphic + presets + AutoEq / Equalizer APO parametric import). No crossfade |
| Lyrics | Navidrome first (`getLyricsBySongId`, synced when available), then LRCLIB, cached |
| Surfaces | Notification and lock screen, Bluetooth and headset buttons, Android Auto, a home-screen widget |
| Look | The desktop's tokens (`crates/app/ui/theme.slint`), the accent from the cover (port of `crates/library/src/accent.rs`), blurred art computed once (never a live blur), a pure-black option for OLED |
| Distribution | Sideloaded APKs, not a store. minSdk 31, targetSdk 35. No proprietary Google libraries (Play services, Firebase): the app is GPL-3.0-or-later, so Cast was dropped |
| Orientation | Portrait and landscape, everywhere |

## How the phone is used, and what follows from it

1. **The screen is off most of the time.** The app is used mostly in a pocket, in a car,
   with Bluetooth. The media session is the product: the notification, lock screen,
   headset buttons, pausing when headphones unplug, ducking for navigation prompts,
   pausing for calls, resuming from Bluetooth or the system's media resumption card
   (`onPlaybackResumption`). All of this has to work before any screen is polished.
2. **The network is unreliable.** The app has to handle tunnels, mobile data and dead
   zones: prefetch whole tracks, keep a cache, offer downloads, and keep playing from
   whatever it already has. Losing the network must not lose the queue.
3. **Sessions are short and one-handed.** A typical session is open, pick something,
   pocket the phone. Frequent actions sit within thumb reach at the bottom of the screen:
   the mini player, the bottom bar and Now Playing's controls. Home shows what is likely
   wanted next (recently played, recently added) without any scrolling.
4. **Landscape happens** (car mounts, a phone on its side at a desk, foldables). Every
   screen has a landscape layout. Width decides the layout, not orientation (window size
   classes). Every screen keeps its state, back stack and scroll position across rotation.

## Battery rules

These apply to every change.

- **Nothing runs while the UI is hidden** except the media service. No polling anywhere.
  Position and progress are drawn by animating from a snapshot (position, time taken,
  speed), not by asking the player every N ms. Collect flows with lifecycle awareness.
- **Audio offload** (`AudioOffloadPreferences`) is on whenever the EQ is off, so the CPU
  sleeps while hardware decodes. ReplayGain is applied as player volume, which keeps
  offload working.
- **Bursty networking**: fetch the current track and the next N (default 2) whole into the
  cache, then let the radio sleep. Hold the Wi-Fi/wake lock only while fetching. Prefer
  the cache over the network on every read.
- **Sized images**: always `getCoverArt` with a `size` for the place it is drawn, in a disk
  cache, never fetched twice. Accent and blurred backdrop are computed once per cover and
  cached.
- **Deferred work** (scrobbles, downloads) goes through WorkManager with constraints.
  Scrobbles are batched and sent when the network is already up.
- Dark by default; the pure-black option makes OLED pixels off.

## Screens

**Navigation.** Compact width: a bottom bar with three tabs, Home, Library and Search.
Settings sits behind a gear on Home's top bar. Medium and expanded width (landscape,
foldables): a navigation rail in place of the bottom bar. Back and predictive back work
everywhere.

**Mini player.** Sits above the bottom bar (or along the bottom of the content area next
to the rail). It shows the cover, title, artist and play/pause, with a progress hairline
on its top edge. Swipe it sideways for previous or next; tap it or swipe it up to open
Now Playing. It is hidden when nothing is queued.

**Now Playing (portrait).** A full-screen sheet over the blurred cover, dismissed by
swiping down. The big cover can be swiped sideways to skip. Below it: title and artist
(tap the artist or the album to go there), a favourite heart, the seek bar, then
shuffle / previous / play / next / repeat in the lower third. The bottom row holds
Lyrics, Queue, Sleep timer and ⋯. Lyrics replace the cover area: synced lyrics
follow playback, tapping a line seeks to it, and plain lyrics scroll. The queue is a
bottom sheet: drag to reorder, swipe to remove, and the current track is pinned.
**Landscape:** the cover is on the left at full height. The right pane holds the title,
seek bar and controls. Lyrics replace the cover on the left, so the controls stay in
reach while reading.

**Home.** The desktop's shelves, from `getAlbumList2`: Recently added as the hero card (its
first album large, with a strip of the next ones), then Recently played, Random albums
(with reshuffle), Most played, Recently released and Favourite albums. Shelves scroll
sideways, and "See all" opens a grid. Pull to refresh. Empty shelves are hidden. In
Settings the shelves can be reordered and shown or hidden; no other shelf editing.

**Library.** Tabs: Albums, Artists, Playlists, Genres. Filter chips: Favourites,
Downloaded. Albums is an adaptive grid (about 2 columns on a portrait phone, 5 in
landscape) with sorts (recently added, name, artist, year, most played, random) and a
fast scroller with a letter bubble. Artists is a list with an alphabet scroller.
Playlists come from the server, read-only in test runs. Navidrome's smart playlists
show up as ordinary playlists.

**Album.** Header: cover, title, artist, year, track count and length. Buttons: Play,
Shuffle, Download (shows its state), Favourite, ⋯. Tracks are grouped by disc; tapping a
track plays the album from it.
**Artist.** Header with the artist image (`getArtistInfo2`, or blurred album art when
there is none). Buttons: Play top songs (`getTopSongs`), Shuffle all. Albums by year, then
appearances.
**Search.** A field at the top that takes focus when the tab is tapped. `search3`,
debounced. Results are grouped into Artists, Albums and Songs. Recent searches are kept.
**Long press** on any album, track, artist or playlist opens one action sheet: Play next,
Add to queue, Go to album or artist, Favourite, Download, Add to playlist. There are no star
ratings: the server's `userRating` is not read and `setRating` is never called.

**Settings** (one short list): Account (server, user, sign out) · Streaming quality (Wi-Fi,
mobile) · Downloads (quality, Wi-Fi only, space used, remove all) · Cache (cap 1/2/4/8 GB,
clear) · Playback (ReplayGain, prefetch count, scrobbling on/off) · Equalizer ·
Appearance (accent from cover or fixed, pure black) · Home shelves · Lyrics (look online)
· About.

**Widget.** Shows the cover, title and artist, with previous, play/pause and next. Built
with Glance and updated only when the track or the play state changes.
**Android Auto.** A browse tree: Home shelves, Playlists, Recently played, Downloads.
Search is supported.

## Architecture

```
android/
  build.ps1              # Windows convenience around Gradle (JDK, SDK, release key, adb install)
  buildSrc/              # build logic: the API scan for :core (part of :core:check)
  version.properties     # the release version (the desktop app's release number)
  tools/emulator.ps1     # optional Windows helper: starts and stops a headless test emulator
  settings.gradle.kts, gradle/libs.versions.toml, gradlew(.bat)
  docs/DESIGN.md         # this document
  core/                  # pure Kotlin/JVM, fast unit tests: Subsonic client + models,
                         # stream URL / quality rules, accent, LRC parsing, EQ math, AutoEq parsing
  app/                   # Android: Compose UI, Media3 service, cache/downloads, widget, Auto
```

- `core` has no Android dependencies, so its tests run on the JVM in seconds. Tests that
  hit a real server are skipped unless `~/.marquee-android/test-server.properties` exists.
- One `PlaybackService : MediaLibraryService` owns the ExoPlayer, cache, prefetcher and
  session. The UI talks to it only through a `MediaController`.
- One data layer: the Subsonic client, an in-memory and on-disk response cache (plain JSON
  files, `files/responses`) so the app opens instantly, then refreshes.

## Testing rules

- **A real server is read-only for tests.** Test runs (debug and benchmark builds, and the
  server tests) use the client's read-only mode: an allowlist of read endpoints
  (`ping`, `getOpenSubsonicExtensions`, `getMusicFolders`, `getArtists`, `getArtist`,
  `getAlbum`, `getSong`, `getArtistInfo2`, `getAlbumInfo2`, `getTopSongs`,
  `getSimilarSongs2`, `getAlbumList2`, `getRandomSongs`, `getSongsByGenre`, `getStarred2`,
  `getGenres`, `search3`, `getPlaylists`, `getPlaylist`, `getPlayQueue`, `getLyricsBySongId`,
  `getCoverArt`, `stream`, `download`, `getScanStatus`, `getUser`). Everything else is
  refused locally and never sent, including scrobble, star/unstar, playlist
  edits, savePlayQueue and playbackReport. A test proves the refusal.
- A test server's credentials live only in `~/.marquee-android/test-server.properties`
  (`url`, `user`, `salt`, `token`, `readOnly=true`). Never copy them into the repo, logs,
  screenshots or commit messages. A debug build can take them through a debug-only path
  (pushed by `adb` to the app's files), never through BuildConfig.
- No real requests to LRCLIB or Last.fm; test with MockWebServer.
- **Automated test runs use an emulator**, not a personal phone: the helper's own AVD
  `marquee-test`, started headless with `-no-window -no-audio -no-boot-anim -no-snapshot-save`
  on port 5580. Every `adb` command names `-s emulator-5580`, and only that emulator is stopped
  (`adb -s emulator-5580 emu kill`).
- Screenshots stay out of the repo.

## Decisions taken while building

The README describes the result.

- **A benchmark build type**: the release code (R8) signed with the debug key, read-only
  and `profileable`, installed as `.bench`, for measuring scrolling and the battery audit.
- **Lyrics order**: server synced, LRCLIB synced, server plain, LRCLIB plain (synced lyrics win
  over the server's plain ones). Not-found answers expire after 14 days. Lyrics are fetched only
  while the view is open, plus the next track once.
- **Downloads** live in their own never-evicted store, counted by what pins them; favourite
  changes and scrobbles go through a durable outbox (one WorkManager job, any network, at most 2 h
  later). Recovery: retry on network errors, skip a track after three failures. A stall seen on a
  very slow link (the player sat in BUFFERING for over a minute after Wi-Fi came back, though the
  prefetch had finished) was a read left hanging on a network that had gone; streams are now
  cancelled on a network change and time out after 12 s.
- **A server error is never stored as a song**: Subsonic servers answer an error on `stream` (a
  wrong password, a song gone) with HTTP 200 and a small JSON or XML body. The stream client refuses
  any 2xx answer whose content type names JSON, XML or text (the desktop client's rule) before Media3
  reads it, so nothing reaches the cache or the downloads; its Subsonic code decides what follows (40
  to 44 pause as a sign-in problem, 70 counts as missing, the rest as unplayable; the download worker
  sets the song aside, or stops on a sign-in error). Entries an older version stored that way (under
  2 KB, or beginning with `{` or `<`) are dropped once per start and fetched again.
- **Sign-out** clears the outbox before the session, then deletes the rest in a process-wide scope
  that no screen can cancel (`Cleanup` in `:core`); a new sign-in waits for it. The outbox keeps a
  hash of the server address and user it was filled for, refuses entries while signed out or for an
  account no longer signed in, and drops another account's entries instead of sending them.
- **Resuming a part**: the download worker and the prefetcher keep each answer's validator (strong
  `ETag`, else `Last-Modified`) in the store's metadata for the song's key and send it as `If-Range`
  when they go on from a part (`Resume` in `:core`, `Resumable` in the app). A whole-file 200 to that,
  another validator, or a 416 (the file got shorter) drops the part and fetches the song from the
  start, once; a part with no validator (stored by an older version) is fetched again. The download
  worker reads the cache's copy only when it is whole. The player's own reads (never stored) still go
  on with a plain range.
- **A transcode cut short**: a live transcode has no length, so a server encoder that stops part way
  ends the reply cleanly and the bytes look like a whole song. After a prefetch or a download of a
  transcoded quality, a copy under a third of what the song's length and bitrate call for
  (`TranscodeCheck`, songs of 20 s or more) is dropped: the prefetch counts as failed, the download is
  set aside.
- **The app's own files** (pins, outbox, favourites, saved queue, saved answers) are replaced whole
  through `SafeFile` (`:core`): a temp file synced to disk, then one atomic rename; a temp file left by
  a failed rename is read while the file is missing. A pins file that is there but damaged is kept as
  `pins.json.bad`, and nothing stored is deleted for not being pinned in that run (songs or covers).
- **Who may control the session**: any app can connect to it. Only the app itself (same uid), the
  controllers the system trusts (Media3's `isTrusted`), the media notification's, Android Auto and
  Automotive, and a few known system packages (system UI, Assistant, Wear's companion) get the browse
  tree, the custom commands (the heart writes to the server; queue edits; sleep timer) and the cover
  grant (`ControllerTrust` in `:core`). Others get the default player commands only.
- **The sleep timer and deep sleep**: its end is on the real-time clock, but its delay follows
  uptime, which stops while a paused phone sleeps. When play is pressed again the timer is checked
  against the clock: past its end it is cancelled (Off), never a sudden pause later; otherwise it is
  armed again for the time really left.
- **A download removed while it plays**: the current song's and the next one's files (the player may
  have opened both) are not deleted under the player when their download is removed or a playlist
  drops them (`HeldDeletes`); they go once they leave the play window or the service stops, unless
  pinned again meanwhile.
- **A pinned playlist that empties**: an answer listing none, or under a quarter, of a pinned
  playlist's songs (8 or more) is held, not followed: its downloads stay until an answer at least
  10 minutes later says the same (`PlaylistShrink`); one that comes back as before clears it.
- **Start-up work, throttled** (`Throttle`): opening the app checks pinned playlists at most once an
  hour (an activity is created again for every rotation; one request per playlist), and a process
  start looks for missing download covers at most once a day (the widget, a media button or a job can
  start the process); the download worker and a new pin fetch their covers anyway. Covers the server
  has no art for are remembered across restarts until the downloads are removed or the user signs out.
- **The equalizer** is scoped to the 10-band graphic EQ and one imported profile; the audio
  session is also handed to system equalizer apps.
- **Auto and the widget**: artwork for other apps comes from a cover content provider, so no URL
  with the token leaves the app. The widget is Glance with no update period.
- **Release**: `build.ps1 -Release` creates the key on first use (RSA 4096, 30 years) in
  `~/.marquee-android/`; the version is the desktop app's release number, in `version.properties`
  or given with `-Pmarquee.version` (the `versionCode` follows from it), and Gradle alone builds, tests and scans
  (`build.ps1` only adds the JDK, the SDK, the key and adb on Windows). The release build has
  normal read-write behaviour (scrobbles, favourites, playlist edits). The read-only UI
  and messages are gated on the build-time `BuildConfig.READ_ONLY`, so R8 removes them from the
  release along with the test-config reader, the LRCLIB stand-in and the read-only guard. The
  release stays https-only (no `http://` option), and test builds never show the server's
  address or the user on screen.
- **Small fixes before release**: downloads keep their covers outside the evictable cover cache;
  the hero card's shelf label has a 48 dp touch target; the long-press sheet stays open across
  rotation (its subject is saved, its callbacks rebuilt); snackbars follow the dark theme.
- **Other phones**: checked on Android 12 (API 31) and 15, 360 x 640 to 800 x 1280 dp,
  three-button navigation, font size 1.3 and 2.0. Android 12 crashed at start: `core` called
  Java 10's `URLEncoder.encode(String, Charset)` (Android has it from API 33), which Android Lint
  doesn't check in a JVM module; it was also used for cover URIs and Auto's ids. `:core:apiScan`
  now catches such calls. Layout fixes: the EQ sliders keep their travel on short screens and
  large fonts (the screen scrolls), Now Playing's landscape fits a 360 dp tall phone and is capped
  at 600 dp wide upright on a tablet, the album cover shrinks so Play stays in the first screen,
  Library's wide header and the settings segments fit large fonts, sheets keep dark-theme system
  bars on a phone in light mode, and the backdrop is capped in brightness so dim text keeps 4.5:1
  over white covers.
- **No Opus-to-Opus transcode**: with an Opus quality chosen, a file that is already Opus
  (suffix `opus`, or `ogg` with a content type naming Opus) at up to twice that bitrate is streamed
  and downloaded as the original. Navidrome's live transcode has no length and no ranges, so the
  track could not seek, and 180 to 128 kbps saved only about 30 %. Unknown bitrates still
  transcode. The queue's and the pins' copies of a song keep its
  suffix, content type and bitrate, and the player's placeholder URI carries them (no token).
- **Seeking in a live transcode**: a track the server transcodes live (no length, no ranges) can't
  seek for as long as it plays, though the prefetcher has it whole within seconds. The session's
  player is a `ForwardingPlayer` (`SeekingPlayer`) around the ExoPlayer, so every seek from any
  controller passes through one place: while the current item can't seek it keeps the seek
  commands (and the song's duration) for the system's controls, and the service decides with
  `SeekReload` (`:core`): stored whole, the item is stopped and prepared again at the position (no
  transition: not a new play, queue, shuffle order and settings untouched; an equal item to
  `replaceMediaItem` would not load again); not yet, the position waits (a newer seek replaces it,
  another track drops it), the seek bar holds it with "Seeking once loaded" and a snackbar says so,
  and the reload happens once the prefetcher has the track. A seek made while a track is still
  opening waits for the player to know whether it seeks, instead of being dropped. `SourcePicker`
  also plays a whole cached copy in the chosen Opus quality rather than stream an original the
  Opus rule asks for.
