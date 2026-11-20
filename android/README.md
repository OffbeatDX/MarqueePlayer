# Marquee Player for Android

A music player for one Navidrome (Subsonic / OpenSubsonic) server, with the desktop Marquee Player's
look: dark, artwork-led, the accent taken from the playing cover. It streams from the server, keeps
what it can on the phone for tunnels and dead zones, and is built to cost as little battery as a
music app can. Kotlin, Jetpack Compose, Media3. The design decisions, their reasons and the battery
and testing rules are in [docs/DESIGN.md](docs/DESIGN.md). This folder is self-contained: nothing outside `android/` belongs to it.

- [Building](#building) · [Installing](#installing-on-the-phone) · [Signing in](#first-sign-in) ·
  [Settings](#settings) · [Downloads and offline](#downloads-and-offline) · [Battery](#battery) ·
  [Android Auto](#android-auto) · [Lyrics](#lyrics) · [Privacy](#privacy) ·
  [Other phones](#on-other-phones-for-friends) · [Reporting a problem](#reporting-a-problem) ·
  [Testing](#testing) · [Project layout](#project-layout) · [Licence](#licence)

## Building

Needs JDK 17 and the Android SDK with platform 35, nothing else. Gradle finds the SDK through
`ANDROID_HOME` or `sdk.dir` in `local.properties` (Android Studio writes that file; it is not in
the repo). The same commands work on Linux, macOS and Windows (`gradlew.bat`, or `.\gradlew` in
PowerShell), from a terminal or from Android Studio's Gradle panel:

```sh
./gradlew :app:assembleDebug       # debug APK
./gradlew check                    # all unit tests, :core's API scan, Android Lint
./gradlew :app:assembleRelease     # release APK (R8), signed when a release key is set up (below)
./gradlew :app:assembleBenchmark   # release code, read-only, profileable
```

The APKs are `app/build/outputs/apk/<variant>/marquee-player-<version>-<variant>.apk` (an
unsigned release ends in `-release-unsigned.apk`). A quicker check than `check`, as CI runs it:
`./gradlew :core:check :app:testDebugUnitTest :app:lintDebug`.

**The API scan.** `:core` is a plain JVM module, so Lint's NewApi check never sees it, and the JDK
has methods Android only gained later (an API 33 call once crashed Android 12 at start).
`:core:apiScan`, part of `:core:check`, reads every java.\*/android.\* call in `:core`'s classes
(with the JDK's `javap`) and fails when one is newer than `minSdk` (in
[gradle/libs.versions.toml](gradle/libs.versions.toml)), using the SDK's
`platforms/android-*/data/api-versions.xml`. Without that file it is skipped with a warning.
The code is in [buildSrc](buildSrc/src/main/kotlin/ApiScanTask.kt); its report is
`core/build/reports/apiScan/apiScan.txt`.

**On Windows**, `build.ps1` is a convenience around the same tasks: it works from any directory,
uses `~/.jdks/temurin-17` when `JAVA_HOME` is unset, writes `local.properties` when it is missing,
creates the release key on the first `-Release`, and runs `adb install`.

```powershell
./build.ps1                        # gradlew :app:assembleDebug
./build.ps1 -Release               # gradlew :app:assembleRelease, signed (stops if it came out unsigned)
./build.ps1 -Test                  # gradlew :core:check :app:testDebugUnitTest (unit tests and the API scan)
./build.ps1 -Release -Install <serial>   # build, then adb install -r on that device
./build.ps1 -Benchmark             # gradlew :app:assembleBenchmark
```

**The release key.** The first `build.ps1 -Release` creates `~/.marquee-android/release.jks` and
`~/.marquee-android/keystore.properties` (keytool, RSA 4096, valid 30 years, `CN=Marquee Player`,
a random password that is never printed). Later builds, `build.ps1` or plain Gradle, sign with the
same key. Elsewhere (CI, another machine), the key can come from four environment variables
instead, which win when set: `MARQUEE_KEYSTORE_FILE` (the path of the `.jks`),
`MARQUEE_KEYSTORE_PASSWORD`, `MARQUEE_KEY_ALIAS`, `MARQUEE_KEY_PASSWORD`. Without either, the
release is left unsigned. Nothing of it is in the repo (`.gitignore` covers `*.jks` and
`keystore.properties`). **Back that folder up**: Android only installs an update signed with the
same key, so losing it means uninstalling the app (and its downloads and settings) to install the
next version. The certificate, for checking an APK with `apksigner verify --print-certs`: SHA-256
`45c802cc9a79e72b7503165b66a64e1255867217c8de04b0a99420d6806d3ec1`.

**Versions.** The app carries the desktop app's release number, `YYYY.M.D.count`: `version` in
[version.properties](version.properties), set by hand to the last release's, or the one given
with `-Pmarquee.version=` (the release workflow gives the tag's). It is the `versionName`; the
`versionCode` Android compares follows from it, the date as YYMMDD and the count in two digits
(`2026.11.20.1` is 26112001), so it grows with every release (Android refuses an update whose `versionCode` is not
higher than the installed one's). Debug builds, tests and other machines work without the key.

**CI.** [.github/workflows/android.yml](../.github/workflows/android.yml) runs, when a version
is tagged (`vYYYY.M.D.count`) and by hand, the unit tests and the API scan, Lint (debug), and the debug
and release (unsigned) builds, and keeps the debug APK and the reports. The release workflow ([release.yml](../.github/workflows/release.yml)) builds the signed APK of
each tagged release, with the release's number as its version, and attaches it to the draft
release; this workflow, run by hand (Actions →
Android → Run workflow, "signed-release"), builds one too. Both need the repository to have
the secrets `MARQUEE_KEYSTORE_BASE64` (the `.jks`, base64-encoded), `MARQUEE_KEYSTORE_PASSWORD`,
`MARQUEE_KEY_ALIAS` and `MARQUEE_KEY_PASSWORD`; without them that job does nothing. The tests
against a real server skip there (no test server is configured).

## Installing on the phone

```powershell
adb install -r app/build/outputs/apk/release/marquee-player-0.1.0-release.apk   # or: ./build.ps1 -Release -Install <serial>
```

The release is `com.offbeat.marquee.player`. The debug build (`.debug`) and the benchmark build
(`.bench`) install beside it as separate apps with their own data; both are read-only test builds
(see [Testing](#testing)), labelled "Read-only" on Home. On Android 13 and later, Android asks once
for notification permission (for the playback controls); Android 12 has no such prompt.

## First sign-in

Server address, user name, password. The address must be **https** (`https://` is added when no
scheme is typed; a path such as `/navidrome` is kept). Plain `http://` is refused by Android, as the
app allows no cleartext traffic. The password is never stored: the app keeps the Subsonic salt and
token, encrypted with a key in the Android Keystore, and backups are off (a restored copy could not
decrypt it anyway). Sign out removes the session, downloads, caches and the saved queue; it finishes
even if you leave the screen at once, and signing in again waits for it. Plays and favourites still
waiting to be sent belong to the account that made them: they are never sent for another one.

## Settings

| Setting | What it does |
| --- | --- |
| Account | "Signed in as *user*", the server's address and Navidrome version (the release build only; test builds show just the version); Sign out (removes downloads, caches and the app's log). |
| Streaming: on Wi-Fi / on mobile data | Original, or Opus at 96–320 kbps. Defaults: original on Wi-Fi, Opus 128 on mobile data. A file that is already Opus at no more than twice the chosen bitrate (up to 256 kbps at Opus 128) plays as it is: the server's live Opus-to-Opus transcode can't be seeked and saves little (a whole copy already cached in the chosen quality still plays instead of streaming the original). Songs of unknown bitrate are transcoded. A live transcode can't seek until the track is whole on the phone (it is fetched first, in seconds); a seek before then holds the seek bar at its place ("Seeking once loaded") and happens as soon as it is, from any control (the app, the notification, Bluetooth, Auto). |
| Downloads: quality, Wi-Fi only | The quality new downloads are fetched in (the same Opus rule applies); whether they wait for Wi-Fi (on by default). Space used, Manage downloads (remove some or all). |
| Cache: size, prefetch ahead | The cap on recent and upcoming tracks (1/2/4/8 GB, never your downloads); Clear. How many upcoming tracks are fetched whole ahead of time (default 2). |
| Playback: ReplayGain | Track, album (when the album plays in order) or off; applied as player volume, so it costs nothing. |
| Playback: Equalizer | 10-band graphic EQ with presets, or an AutoEq / Equalizer APO parametric profile. Off by default; costs battery while on (below). |
| Playback: Scrobbling | Tells the server what you played (its play counts, and Last.fm if the server is set up for it). |
| Appearance | Accent from the cover, or a fixed one; pure black for OLED screens. |
| Home: Edit shelves | Order and show/hide Home's shelves. |
| Lyrics: Look up online | Ask LRCLIB when the server has no lyrics (on by default). |
| About | Version, server, the app's licence (GPL-3.0-or-later), the source code's address once one is set, Share diagnostics (below), the open-source licences with the GPL and Apache texts. |

## Downloads and offline

- Download (or long-press → Download) an album, a playlist or a song. Downloads are WorkManager
  work: they wait for Wi-Fi while "Wi-Fi only" is on, survive restarts, and resume a half-fetched
  song where it stopped, as long as the server's file is still the same one (a song replaced on the
  server since is fetched again from the start). A notification shows progress.
- Downloaded songs live in their own store (`files/downloads/media`), apart from the size-capped
  cache (`cache/media`); the player reads downloads, then the cache, then the network. A song shared
  by several downloads is stored once and goes when the last one is removed.
- Each download keeps its covers with it (`files/downloads/covers`: the album's cover at every size
  it is drawn at, its songs' covers at two), outside the cover cache, so clearing that cache or the
  system trimming it never leaves a downloaded album without art offline. Removing the download
  removes them.
- Downloaded playlists follow the server: checked when the app opens, when the playlist is opened,
  and once a day while charging on Wi-Fi.
- Offline, Library's **Downloaded** chip and the Downloads list show what plays; anything not on the
  phone is dimmed, and the queue skips it. Favourites and scrobbles made offline wait in an outbox
  and are sent in batches once the network is back.

## Battery

- **Audio offload** is on whenever the equalizer is off: the phone's audio hardware decodes and the
  CPU sleeps between buffers (Media3 also lets go of its wake lock while offloaded). ReplayGain is
  applied as player volume, which keeps offload working.
- **The equalizer costs battery while on**: it runs on the CPU (double-precision biquads), so
  offload is off and the CPU wakes for every buffer. Off, it costs nothing.
- **Prefetch is bursty**: the current track and the next two are fetched whole into the cache, then
  the radio sleeps until the next track starts. The network wake lock is held only while fetching.
- **Nothing ticks in the background**: the seek bar and lyrics are drawn from a position snapshot
  and stop when nothing is drawn; there is no polling. Covers are fetched at the size they're drawn
  at, once; the accent and backdrop are computed once per cover.
- **What runs in the background** (besides playback): the scrobble/favourite outbox (`outbox`, one
  job when something waits, needs a network, at most 2 h later) and, with a playlist downloaded,
  the playlist check (`downloads-playlist-sync`, daily, Wi-Fi and charging). Downloads run only
  while something is missing. No alarms, no periodic timers.

The audit (emulator, benchmark build, before the first release; an emulator has no offload hardware, so its CPU
numbers are what a phone pays with the EQ on, not what it pays with offload):

| Scenario (screen off) | Wake locks | Network | CPU | Other |
| --- | --- | --- | --- | --- |
| Downloaded album playing, 5 min | ExoPlayer's audio lock (and the system's `AudioMix`), all along; no network lock | 0 bytes; no Wi-Fi lock | 34.5 s (6.7 s/min, 94 % kernel: the emulated audio path) | 2 frames (the screen turning off), 0 alarms, 0 jobs |
| Streamed album playing, 7 min | the same; the network wake mode 3.7 s in all | 12.9 MB in 2.6 s at the start (this track and the next two), then one 0.5 s burst (about 3 MB) per new track, quiet in between; Wi-Fi lock 0.5 s | 61 s (8.7 s/min) | 1 frame, 0 alarms, 0 jobs |
| Paused, 11 min | none | 0 bytes | 0.6 s in all | foreground dropped at the pause; Android stopped the idle service after 1 min, which saved the queue and released the player and session; the notification went, the system's resume card stayed. 0 frames, 0 alarms, 0 jobs |
| Not playing, in the background | none | none | none | 0 alarms, 0 jobs (the outbox and playlist jobs exist only while something waits or a playlist is downloaded) |

On a phone with offload, the CPU and the audio wake lock mostly sleep while playing; the emulator
can't show that. The audit found nothing against the battery rules to fix.

## Android Auto

Auto browses the playback service: Home's shelves, Playlists, Recently played, Downloads; search
works (offline: the downloads), and the heart is a custom action in Auto's player. A sideloaded app
only shows up in Auto with Auto's developer settings → **Unknown sources** on (tap the version in
Auto's settings ten times to get the developer settings). Artwork goes to Auto as
`content://…covers/…` URIs from the app's cover provider, never as URLs with the token. Other apps
that connect to the player (anything not the system, Auto, Assistant or Wear) can only play, pause and
skip: they get no browse tree, no covers and no heart.

## Lyrics

The server first (OpenSubsonic `getLyricsBySongId`, synced when it has them), then
[LRCLIB](https://lrclib.net) (synced first, then plain), when "Look up online" is on. Answers are
cached on the phone, including "none", so a song is looked up once. Synced lyrics follow playback
and a tap seeks to the line.

## Privacy

- **Stored on the phone, in the app's private storage:** the session (salt and token, encrypted with
  a Keystore key), settings (SharedPreferences: `session`, `ui`, `playback`, `library`, `eq`,
  `downloads`), the last server answers (`files/responses`), the queue, favourites and the outbox,
  downloads and their covers (`files/downloads`), the caches (`cache/media`, `cache/covers`,
  `cache/lyrics`, `cache/art`). App backups are off. Sign out deletes all of it.
- **Sent anywhere:** only to your server; to LRCLIB (artist, title, album and length of the song
  whose lyrics are missing) while "Look up online" is on. Nothing else: no analytics, no crash reporting. The app's log
  (below) stays on the phone unless you share it.

## On other phones (for friends)

Any phone with Android 12 or later. It has been checked on emulators at Android 12 and 15, small
(360 x 640 dp) to tablet (800 x 1280 dp) screens, three-button and gesture navigation, both
orientations and font sizes up to 2.0; on real hardware, only on a Pixel.

- **Installing:** open the APK from Files (or the browser's downloads) and allow that app to install
  unknown apps when asked. Play Protect may warn about an unknown developer: "Install anyway".
- **Samsung (One UI):** Samsung puts apps it thinks are unused to sleep, and a sleeping app's music
  stops in the pocket and Bluetooth play/pause stops reaching it. Settings → Apps → Marquee Player →
  Battery → **Unrestricted**; and in Settings → Battery → Background usage limits, make sure it is not
  under "Sleeping apps" or "Deep sleeping apps" (add it to "Never sleeping apps"). After an update,
  check once that it is still there.
- **Xiaomi, OnePlus, Oppo, Vivo, Huawei** do the same under other names (autostart, battery saver
  per app, "allow background activity"): set Marquee Player to no restrictions.
  [dontkillmyapp.com](https://dontkillmyapp.com) has each maker's steps.
- What it looks like when it bites: playback stops a few minutes after the screen goes off, or the
  headset button does nothing until the app is opened. A Pixel doesn't do this.
- If something goes wrong, Settings → About → Share diagnostics (below) says which phone and Android
  version it was.

## Reporting a problem

Settings → About → **Share diagnostics** builds one text file and opens the share sheet, so a
friend can send it to you (mail, Messages, Drive…) when something goes wrong. Nothing is sent
unless they share it; there is no telemetry.

- **What it holds:** the app version and build type; the phone's maker, model, Android version,
  screen size and density; the Navidrome version; the network at the time; every setting's value;
  download and cache sizes; the last crash, if any; and the app's log.
- **The log** (`files/logs`, every build type): about 500 KB in two rotating files, written off the
  main thread. Playback state changes and errors, network changes, recovery, downloads and
  background jobs, warnings and errors; nothing per frame or per poll. Logcat gets the same lines.
- **Crashes:** an uncaught exception is written to the log and to `files/logs/last-crash.txt`, then
  Android handles it as usual. The next report includes it.
- **Scrubbed twice**, as each line is written and again when the report is built: every URL becomes
  `<url>`, any `u=`, `t=`, `s=`, `p=` value is hidden, and the token, salt, user name and server host
  (and its domain), also percent-encoded or in punycode, become `<token>`, `<salt>`, `<user>`,
  `<server>`. A user name of one or two letters is only replaced as a whole word.
- Sign out deletes the log and the last crash.
- Debug only, for checking the handler: `adb -s emulator-5580 shell am broadcast -n
  com.offbeat.marquee.player.debug/com.offbeat.marquee.player.harness.CrashTestReceiver` crashes the
  app on purpose.

## Testing

Test builds never write to a server: debug and benchmark builds are **read-only**. Only the read
endpoints listed in [docs/DESIGN.md](docs/DESIGN.md#testing-rules) can be called; scrobbles,
favourites and playlist edits are refused on the phone before anything is sent, and LRCLIB
is off (a local stand-in on 127.0.0.1 can be pushed instead). The release build has none of this: the read-only code paths, the test-config reader, the
LRCLIB stand-in, the Auto test harness and the loopback network rule are compiled out of it.

`tools/emulator.ps1` is an optional Windows (PowerShell) helper, not needed to build or test: any
emulator or phone with `adb install -r` of the debug APK works as well. What it adds is a dedicated
headless emulator and pushing the test config into the debug or benchmark build:

```powershell
./tools/emulator.ps1 start                  # AVD marquee-test, headless, port 5580 (-Gpu swiftshader_indirect if qemu crashes)
./build.ps1 -Install emulator-5580
./tools/emulator.ps1 push-test-config       # the debug build signs in to the test server by itself
./tools/emulator.ps1 push-test-config -Benchmark   # the same for the benchmark build (via adb root)
./tools/emulator.ps1 stop
```

- The test server is `~/.marquee-android/test-server.properties` (`url`, `user`, `salt`, `token`);
  it never goes in the repo. The `core` tests against a real server run only when it exists, and
  read-only; otherwise they report as skipped (CI, other machines).
  `./gradlew :core:test -Pmarquee.testUserHome=<an empty folder>` runs them as if it did not exist.
- `core` tests are plain JVM tests (MockWebServer for HTTP): the Subsonic client in both modes, the
  read-only allowlist, quality rules, prefetch and recovery rules, pins and their covers, the outbox,
  LRC parsing and LRCLIB matching, the EQ maths and APO import, the accent, Auto's browse tree.
- The benchmark build is the release build (R8, not debuggable) signed with the debug key, read-only,
  `profileable`: for start-up, scrolling (`dumpsys gfxinfo …bench`) and the battery audit.
- Debug only: `AutoHarnessActivity` stands in for Android Auto (`adb -s emulator-5580 shell am start -n
  com.offbeat.marquee.player.debug/com.offbeat.marquee.player.harness.AutoHarnessActivity --es query <text>`);
  a file `files/debug-no-stall-fix` turns off the network-stall fixes for comparisons.

## Project layout

```
android/
  version.properties                the release version
  docs/DESIGN.md                    the design decisions and the battery and testing rules
  build.ps1                         the Windows convenience around Gradle (JDK, SDK, release key, adb)
  buildSrc/                         build logic: the API scan for :core
  tools/emulator.ps1                the test emulator (optional, Windows)
  core/                             pure Kotlin/JVM, unit-tested
    subsonic/                         the client (token auth, read-only allowlist, models, errors)
    playback/ offline/ sync/          quality, prefetch and recovery rules; pins and covers; the outbox
    library/ lyrics/ eq/ art/         shelves and sorts; LRC and LRCLIB; the EQ maths; accent and blur
    auto/ settings/                   Auto's browse tree; settings models
    diag/                             the log scrubber, the ring log, the crash handler, the report
  app/                              the Android app (com.offbeat.marquee.player)
    playback/                         PlaybackService (MediaLibraryService), sources, cache, prefetcher, EQ sink
    offline/ sync/                    downloads, their worker and cover store; the outbox worker
    data/ session/ net/               the data layer and response cache; sign-in; network state
    diag/                             the app log (logcat + file), Share diagnostics
    art/ widget/                      covers (Coil, the cover provider), accent; the Glance widget
    ui/                               Compose screens: home, library, album, artist, search, player, settings
    src/debug, src/release            the test-only parts and their release stubs
```

## Licence

Copyright © 2026 Offbeat. Like the rest of the repository, the app is under the GNU General Public
License, version 3 or later ([LICENSE](../LICENSE)), with no warranty. The libraries it is built
with keep their own licences (all Apache License 2.0): Settings > About > Open-source licences lists
them and shows the GPL and Apache texts in full. The app links no proprietary Google library (no
Play services, Firebase or Cast). The "Source code" row in About opens `SOURCE_URL`, set in
`app/build.gradle.kts`.
