# Releases

Version numbers, the release command, the editions it packages and the size of the exe.

## Version and releases

The version is `YYYY.M.D.count`: the build date
(local time) and that day's release count (`2026.11.15.1`, `.2`, ...). `crates/app/build.rs`
makes it and passes it to the code as `APP_VERSION` (`about::VERSION`). Only a release build
takes a number: one made with `MP_RELEASE_BUILD=1` (`cargo xtask release` sets it) in a profile
that inherits from `release`. Every other build (`cargo build`, `cargo test`, clippy,
`cargo xtask build`) is `YYYY.M.D-dev` and numbers nothing, so tests and quick builds never use up
release numbers. `crates/app/.build-version` keeps the date and count, `.app-version` the
last release's full number; both are git-ignored. The build script runs again whenever
`MP_RELEASE_BUILD` changes, so a release build always gets its number; if nothing at all
changed since the last release, the exe is the same and so is its number.

The version shows in Settings > About, the exe's Properties > Details (file and product
version, product name, description, company "Offbeat"; a version resource written by
`build.rs` beside the icon, marked pre-release for a dev build), the log's first line, the
tray tooltip while nothing is loaded, and the client name sent to Last.fm
(`MarqueePlayer/<version>`) and LRCLIB (`MarqueePlayer/<version> (desktop music player)`).

The Windows, Linux and macOS release builds of the same sources get the same number: when its
build script numbers a release, it fingerprints everything that goes into the binary (every
crate's sources, manifests and build scripts, the UI markup, the patched dependencies,
`Cargo.lock`, carriage returns dropped, and the compiler's version) and keeps the last
release's number (date and count) while that fingerprint is unchanged; `.build-version` holds
it (`YYYY.M.D count fingerprint`). So `cargo xtask release` on Windows and in WSL on the same
checkout, one after the other in either order (each in its own target folder), number their
builds alike, as does running either again with nothing changed. A change to any source
is a new number. They must run on the same checkout (WSL reaches the Windows one under
`/mnt/c`): the state files are git-ignored, so another clone numbers on its own, and a fresh
clone (a CI job) has none: its first release is today's date and count 1.
`.github/workflows/release.yml` runs the [checks before a push](building.md#checks-before-a-push) on the systems it
builds, then `cargo xtask release --skip-checks` on Windows, on Ubuntu 22.04 and on macOS
(`macos-latest`, Apple Silicon), and keeps the zip, the installer, the AppImage, the disk image
and their symbols as the run's artifacts, after checking that every system built the same
number. It runs:

- **on a tag** `v<number>`: all three systems; it seeds `.build-version` so the build gets the
  tag's number, which must be dated today (UTC), and fails if a build got another. The run
  ends by making a **draft** GitHub release named "Marquee Player <number>" for the tag, with
  the four packages attached (not the symbols, which hold the build machines' paths and stay
  the run's artifacts) and notes generated from the commits since the last release. Nothing is
  public until the owner publishes the draft from the repository's Releases page. Pushing the
  tag again after a failed run (delete it, fix, tag again) updates the draft's files rather
  than failing; a release already published is never changed;
- **by hand** (Actions > Release > Run workflow): with an optional release number (empty:
  today's .1) and a tick box per system, all ticked by default. A system left out is neither
  checked nor built, and the number comparison takes only those built (a macOS runner's
  minute costs ten Linux ones on a private repository). It only keeps the artifacts: no
  GitHub release is made. "A test build" (`skip-checks`) packages without running the checks
  first, for trying a package (a build to hand to a tester); a tag's run always checks.

The same run builds the Android app's signed APK (`MarqueePlayer-<version>-android.apk`, with
the release's number as its version), and a tag's draft release carries it, when the repository
has the four `MARQUEE_*` signing secrets ([android/README.md](../android/README.md), "CI");
without them the release is made without an APK, and says so in a warning.

Only the draft-release job may write to the repository (`contents: write`); every other job
reads. The packages are unsigned but for the macOS bundle's ad-hoc signature ([macOS > The
package](macos.md#the-package)).

`cargo xtask release` (`crates/xtask/src/release/`) makes the release for the system it runs
on: on Windows the portable zip and the installer, on Linux (in WSL too) the AppImage, on
macOS the disk image, all in `dist/`. It asks nothing and exits non-zero on any failure, before anything is packaged, so it
runs unattended (CI); its last lines name every artifact with its size. Options: `--skip-checks`
(no tests and no clippy: for trying the packaging, and for CI, which runs them as a job of its
own; the output says they were skipped), `--target-dir DIR` and `-j N` as for every command.
In order, on every system:

1. refuses at once what would fail later: on Windows, `MarqueePlayer.exe` beside Cargo.toml or
   `<target>\dist\MarqueePlayer.exe` in use (quit the app from its tray icon); and finds what
   packaging needs, saying what to install when one is missing. Windows: Inno Setup 6.3 or newer
   (`MP_INNO_SETUP`, its folder, else where its installer puts it, for the user or for
   everyone), the Visual C++ runtime DLLs of the newest Visual Studio's
   `VC\Redist\MSVC\<version>\x64\Microsoft.VC14x.CRT` (`MP_VC_REDIST` to choose another folder),
   `tar.exe` and Windows PowerShell (both part of Windows). Linux: libpipewire 0.3.65 or newer to
   build against (`MP_PIPEWIRE_PREFIX`, default `~/.local/share/mp-build/pipewire-0.3.65`, made
   by `packaging/linux/pipewire-sdk.sh`), pkg-config, desktop-file-utils, appstream, patchelf,
   binutils, curl, the fallback font and the libraries the AppImage carries; it downloads
   appimagetool 1.9.1 and the AppImage runtime (pinned, SHA-256 checked) into
   `MP_APPIMAGE_TOOLS` (default `~/.local/share/mp-build/appimage-tools`). macOS: the Xcode
   command line tools (`codesign`, `hdiutil`, `dsymutil`, `strip`, `otool`, `plutil`, `ditto`,
   taken from `/usr/bin`);
2. `cargo test --workspace --release` and `cargo clippy --workspace --all-targets -- -D
   warnings`, each without and with the bench harness (`--features marquee-player/bench`), so
   code that compiles only one way cannot rot: nothing is built or packaged from a tree that
   fails them (on Linux with `TMPDIR` in the target folder, for the tag editor's tests);
3. writes `<target>/release-remap.toml`: `--remap-path-prefix` flags for the build machine's home
   folder, cargo and rustup homes and the checkout (and the C compilers' counterpart in CFLAGS,
   with libopus rebuilt when those change), so the user name stays out of the binary; on Linux
   also `RUNPATH=$ORIGIN/../lib`. It builds with `MP_RELEASE_BUILD=1` in the `dist` profile
   (`Cargo.toml`: release plus line tables, in `<target>/dist`, apart from `cargo xtask build`'s
   `release`); the remapped build compiles everything the first time, several minutes;
4. checks `.app-version` against the binary's own `--version` (and, on Windows, its version
   resource), so the binary, the packages' names and the symbols folder always agree;
5. searches what ships for the home folder, the checkout's path and the user name (UTF-8 and
   UTF-16, case ignored); a find stops the release;
6. Windows: copies the exe to `MarqueePlayer.exe` beside Cargo.toml and keeps the exe and its PDB
   in `dist\symbols\<version>\`; Linux: splits the debug symbols into
   `dist/symbols/<version>/linux/MarqueePlayer.debug` (with the stripped binary beside it, which
   carries a debug link to it), prints the newest glibc and libstdc++ symbol versions it uses and
   refuses a linked library it does not know how to treat; macOS: keeps the binary and its
   `MarqueePlayer.dSYM` in `dist/symbols/<version>/macos/`, refuses a linked library that is not
   part of macOS and a binary built for a macOS newer than 11.0. Never shipped;
7. writes `THIRD-PARTY-NOTICES.txt`: every package linked into the binary (from `cargo metadata`
   for the target, the workspace's own packages left out) with its licence files, plus Skia,
   libopus, SQLite, and what the package carries besides (the runtime DLLs; the AppImage's
   libraries and font). A package that ships no licence file gets the standard text of its
   licence (the REUSE templates in Slint's checkout, or another package's text that names no
   copyright holder) and its own authors as the holders. Licence texts that must be there and
   are missing stop the release;
8. Windows: `dist\MarqueePlayer-<version>-portable.zip`: the exe, the three Visual C++ runtime
   DLLs, `portable.txt` (`packaging\windows\portable.txt`), `README.md` (the user-facing
   `packaging\user-README.md`, not the repository's README), `LICENSE.txt` (`packaging\LICENSE.txt`) and the
   notices, after `packaging\windows\check-deps.ps1` has read every import of the exe and the
   DLLs (normal and delay-loaded) and found each in the folder or part of Windows; then
   `dist\MarqueePlayer-<version>-setup.exe` with Inno Setup (`packaging\windows\MarqueePlayer.iss`)
   from the same files without `portable.txt`, its icon made from the exe's artwork (the
   `windows_icon` example), checking that the installer's version resource says the same
   version. Linux: the AppDir, the desktop file validated (`desktop-file-validate`) and the
   metainfo (`appstreamcli validate --no-net`, its release set to this version; a known
   warning, `url-homepage-missing`: there is no homepage yet, and the metainfo leaves the URL out
   rather than make one up; add `<url type="homepage">` once there is one), then
   `dist/MarqueePlayer-<version>-x86_64.AppImage`, started once
   (`--appimage-extract-and-run --version`). macOS: `MarqueePlayer.app` (Info.plist and the
   `.icns` written by the release, the stripped binary, the README, licence and notices in
   `Resources`), signed ad hoc (or with `MP_CODESIGN_IDENTITY`), verified and started once
   (`--version`), then `dist/MarqueePlayer-<version>-macos-arm64.dmg` (`x86_64` on an Intel Mac)
   with `hdiutil` ([macOS > The package](macos.md#the-package)).

Each system makes its own packages: the Linux build takes as long again, needs a Linux system
with its own packages, and fails on its own terms; it is not worth holding the Windows release
hostage to. In WSL the target folder defaults to `~/.cache/mp-release-target` (on the Linux file
system: under `/mnt/c` cargo is several times slower) when neither `--target-dir` nor
`CARGO_TARGET_DIR` names one.

The release machine for Linux is an Ubuntu 22.04 (a WSL distribution does: `wsl --install -d
Ubuntu-22.04 --no-launch`, then `wsl -d Ubuntu-22.04 -u root`), with, besides the build
packages of [Linux > Building](linux.md#building) (without `libpipewire-0.3-dev` and `libspa-0.2-dev`, too old
there) and rustup with 1.99.1: `meson ninja-build libdbus-1-dev` (for
`packaging/linux/pipewire-sdk.sh`, run once), `desktop-file-utils appstream patchelf
libxkbcommon-x11-0 libxcb-xkb1 fonts-dejavu-core ffmpeg curl`.

## Editions

| | Portable (Windows) | Installed (Windows) | AppImage (Linux) | Disk image (macOS) |
| --- | --- | --- | --- | --- |
| File | `MarqueePlayer-<v>-portable.zip` | `MarqueePlayer-<v>-setup.exe` | `MarqueePlayer-<v>-x86_64.AppImage` | `MarqueePlayer-<v>-macos-arm64.dmg` |
| Recognised by (`edition.rs`) | `portable.txt` beside the exe | the installer's registration naming this exe | `$APPIMAGE` (set by the AppImage runtime) | none: every copy of the bundle is standalone |
| Data | `Data\` beside the exe (all of it, caches in `Data\cache`) | `%APPDATA%` / `%LOCALAPPDATA%` | the XDG folders; a `<name>.AppImage.home` folder beside it makes it portable | `~/Library/Application Support/MarqueePlayer`, caches in `~/Library/Caches/MarqueePlayer` |
| Writes to the registry | nothing (unless "Start with Windows" is turned on) | the installer's registration; the app only the Run value | n/a | n/a (a LaunchAgent file when "Open at login" is on) |
| Settings > Window | Start with Windows (with a warning that it stores the path) | Start with Windows, Default app for music files | neither | Open at login |

About names the edition under the version, and the bug report line Copy makes says it in
brackets ("Marquee Player 2026.11.16.1 (portable) · Windows 11 ..."). An exe that is none of
these (a build, the copy `cargo xtask release` leaves in the checkout, an exe copied on its own)
is "standalone": data in the profile, as before the editions, nothing registered.
`MP_DATA_DIR` overrides the data folder in every edition. Everything Windows-specific is in
`platform/windows/integration.rs`, the Linux side in `platform/linux/integration.rs`, the macOS
side in `platform/macos/integration.rs`.

### Portable (Windows)

- **The marker** is a file, `portable.txt`, beside `MarqueePlayer.exe` (the zip has it; it says
  in a few lines what it does and that deleting it makes the exe use the profile). A file, not a
  `Data` folder: an emptied or deleted data folder must not silently send the library to the
  profile, and the folder is made on the first start. Without the marker the exe behaves as it
  always has (the copy in the checkout relies on it).
- **Everything** goes to `Data\` beside the exe, in the `MP_DATA_DIR` layout (settings, theme,
  library database, log, tag-edit journal, Last.fm and scrobble files, art and lyrics caches).
  Nothing is written to the profile or the registry by the app, unless the user turns on
  "Start with Windows" (the Run value names the exe; the setting's own hint says so). Windows
  itself still keeps its usual traces (file dialogs' recent folders, the GPU driver's shader
  cache).
- **A folder it cannot write to** (read-only medium, a protected folder) is refused at start
  with a message naming the folder and the error, and what to do (copy the folder elsewhere, or
  delete `portable.txt`): never a silent fallback to the profile. `edition::check_writable`
  probes every data folder before anything else is opened.
- **Drive letters.** The database knows a track by its absolute path. The portable edition
  remembers the root of the drive it ran from (`portable_volume` in `settings.json`: `E:\` or
  `\\server\share\`, as the exe's path names it; not `GetVolumePathNameW`, which answers a
  `subst` drive with a folder inside it). When that changes (the stick is F: on another
  PC), each library folder that was on the old drive, is gone there and exists at the same
  place on the new drive, follows it: `library::reroot` rewrites, in one transaction, every
  track's path, every album's folder, the group folders and the untagged albums' keys, so ids,
  plays, ratings, playlists and the queue stay as they are and nothing is re-read or re-made.
  Folders on other drives (a music disk that stays D:) are left alone. Not followed: the tag
  editor's undo journal (an undo across a letter change says the file is not there).
- **Single instance**: keyed by the data folder as before, so a portable copy and an installed
  one run side by side, each with its own library. About's file locations and Open folder show
  `Data\`.

### The Visual C++ runtime

The exe imports `MSVCP140.dll` and `VCRUNTIME140.dll`, and `msvcp140.dll` imports
`VCRUNTIME140_1.dll` (read from the files' import tables by `packaging\windows\check-deps.ps1`; Skia's prebuilt
library needs the dynamic C++ runtime, so there is no static build). All three are shipped
**app-local**, beside the exe, in both Windows editions: Microsoft supports deploying the
redistributable DLLs this way ("Redistributing Visual C++ files", app-local deployment; the
files of the Visual Studio `Redist` folder are Distributable Code under its licence, which asks
that the end user's terms protect them at least as much: `LICENSE.txt` says they are
Microsoft's, not under the GPL, and not to be modified or reverse engineered. For the GPL they
are System Libraries, which it does not ask to be released as source). The loader
looks in the exe's folder first for any DLL that is not a KnownDLL (these are not), so the
copies beside the exe are the ones loaded, whatever is or is not installed; everything else the
exe and the three DLLs import is part of Windows 10 and 11 (the Universal CRT, `api-ms-win-*`,
Direct3D, DirectWrite...). Preferred over running Microsoft's redistributable from the
installer because that needs administrator rights, which a for-me-only install does not have,
and the portable edition has no installer. The cost: Windows Update does not service app-local
copies, so each release ships the runtime of the Visual Studio it was built with (the newest
in its Redist folder); a newer copy installed in System32 is not used by this app.

### Installed (Windows)

**Inno Setup 6** (6.7.4 used; `packaging\windows\MarqueePlayer.iss`): free (its licence asks
commercial users to buy a licence; this app is given away), the conventional tool, and it has
exactly the scope choice asked for (`PrivilegesRequiredOverridesAllowed=dialog`): the first page
asks "Install for me only" (the default: no administrator rights, `%LOCALAPPDATA%\Programs\
Marquee Player`) or "Install for all users" (elevates, `C:\Program Files\Marquee Player`). An
update finds the earlier installation and keeps its scope without asking
(`UsePreviousPrivileges`). Windows 10 1809 or newer, x64 (and Windows 11 on ARM, emulated).

What it installs: the exe, the runtime DLLs, `README.md`, `LICENSE.txt` (shown as the licence
page too), `THIRD-PARTY-NOTICES.txt`; a Start menu shortcut; a desktop shortcut (a task,
unchecked); the Apps & features entry (name, publisher Offbeat, version, icon, size, uninstaller).

What it registers, in HKCU for a for-me-only install and HKLM for all users (Inno's `HKA`),
following Microsoft's "Default Programs" and "file types" documentation, and taking no
association:
- a ProgID per format, `MarqueePlayer.<ext>` ("FLAC audio", ...), with its icon and
  `shell\open\command` `"...\MarqueePlayer.exe" "%1"`, listed in each extension's
  `OpenWithProgids`: the app is under "Open with" for every format the scanner reads
  (`library::scan::AUDIO_EXTS`; a unit test checks the script's list against it);
- `Software\Classes\Applications\MarqueePlayer.exe` with `FriendlyAppName`, `SupportedTypes`
  and its open command, which is also how the app knows it is the installed edition: the
  registration's command names its own exe;
- `Software\Offbeat\Marquee Player\Capabilities` (name, description, icon, `FileAssociations`)
  under `RegisteredApplications` as `Marquee Player`: the app's page in Settings > Default apps;
- App Paths, so Win+R `MarqueePlayer` starts it.

**Default app.** Windows does not let a program make itself the default (since Windows 8 the
choice is the user's, protected by a hash an installer cannot write). The Finished page has an
unchecked "Make Marquee Player the default for music files (opens Windows' Default apps
settings)", and Settings > Window > Default app for music files has "Choose default apps…",
both opening `ms-settings:defaultapps?registeredAppUser=Marquee%20Player` (or
`registeredAppMachine=` for an all-users install): on Windows 11 21H2 with the 2023-04 update
or later, Marquee Player's own page there, where "Set default" takes every type at once or
each type on its own. Windows 10 ignores the parameter and opens Default apps itself (choose by
file type, or "Set defaults by app"). The button shows only in the installed edition.

**Start with Windows** (Settings > Window, off unless turned on; Windows editions only): the
user's `HKCU\...\CurrentVersion\Run` value `Marquee Player` (`Marquee Player (portable)` for the
portable edition, so the two never overwrite each other) with `"<exe>" --minimized`, which
starts the app in the tray: the window is made but not shown (a second launch or the tray brings
it back, at its saved size, maximized if it was). The switch reads the registry, so it is on
only when the value starts this exe and Task Manager's Startup apps has not turned it off
(`StartupApproved\Run`; turning it on here clears that). Turning it off deletes the value.

**Running copy.** The installed edition holds a mutex, `MarqueePlayer.Installed` (in the
session and as `Global\`), and waits on an event, `Local\MarqueePlayer.Installed.Quit`. Setup
(on the Preparing page, so a copy started while the wizard was open counts) and the uninstaller
find the mutex, say they will close it (silently: without asking), set the event, and wait up
to ten seconds: the app quits the normal way (queue, position, settings saved). If it does not,
they ask to quit it from the tray and Retry. An all-users install also looks for copies in other
users' sessions (the `Global\` mutex) and offers Abort, Retry or Ignore (Ignore: Windows
replaces files in use at the next restart). The mutex is not keyed by data folder, unlike the
single-instance one, because setup cannot compute that key; it is held only by the installed
edition, so portable and standalone copies never hold setup up. Restart Manager stays on as a
fallback (`CloseApplications`, without restarting); for it, and for Windows shutting down, the
app now answers `WM_ENDSESSION`: it saves the queue, position and settings before returning,
and quits when only it was asked to close (`ENDSESSION_CLOSEAPP`).

**Upgrades** install over the earlier version in place (same `AppId`), keep the scope and the
data (which is in the profile, not the install folder), and re-register the same names.
**Uninstall** removes the files, shortcuts and every key and value above (Inno's
`uninsdeletekey` / `uninsdeletevalue` on what it wrote; other programs' entries under an
extension are left alone), the Run value when it starts this installation's exe, and the empty
install folder. Then it asks "Also delete your Marquee Player library and settings?" (default
**No**; never in a silent uninstall), naming the two folders; Yes deletes `%APPDATA%\
MarqueePlayer` and `%LOCALAPPDATA%\MarqueePlayer`. Music folders are never touched.

**For me and for everyone on one PC.** Setup refuses an all-users install while this user has a
for-me-only one, and a for-me-only install while an all-users one exists, saying which to choose
to update or that the other must be uninstalled first (the library is kept). Two installations
would both register the app, the user's hiding the machine's, and share one library in the
profile anyway; one per user is simpler to support. (Another user's for-me-only installation
cannot be seen from this account; each user's own registration wins for them.)

**Silent install and uninstall** (Inno's standard switches):

    MarqueePlayer-<v>-setup.exe /VERYSILENT /SUPPRESSMSGBOXES /NORESTART /CURRENTUSER
    MarqueePlayer-<v>-setup.exe /VERYSILENT /SUPPRESSMSGBOXES /NORESTART /ALLUSERS     (elevated)
        [/DIR="C:\Some\Folder"] [/TASKS="desktopicon"] [/LOG="setup.log"]
    "%LOCALAPPDATA%\Programs\Marquee Player\unins000.exe" /VERYSILENT /SUPPRESSMSGBOXES /NORESTART
    "C:\Program Files\Marquee Player\unins000.exe" /VERYSILENT /SUPPRESSMSGBOXES /NORESTART     (elevated)

`/SILENT` shows progress only. A silent install closes a running copy without asking, does not
start the app or open Default apps; a silent uninstall keeps the library data.

**Unsigned.** There is no code signing, so SmartScreen warns about the installer and the exe
("Windows protected your PC" until enough people have run that exact file). Everything else that
an unsigned binary can do to look like what it is: complete version resources on both (company
and publisher Offbeat, product Marquee Player, description, the same version, copyright), an
application manifest in the exe (`asInvoker`, Windows 10 and 11 declared, common controls 6), the
same icon, no packer or self-extractor tricks (Inno's own loader; the exe is plain), and the user
README says plainly what the warning is and how to proceed.

### Windows 10

Read from the code and Microsoft's documentation; not run on Windows 10 (no Windows 10 machine,
VM or Sandbox was at hand: see `packaging/windows/FIRST-INSTALL-TEST.md`):
- Needs nothing newer than Windows 10 1809 that was found: Direct3D 12 (wgpu; else the software
  renderer), WASAPI shared and exclusive, MMCSS, the media overlay (SMTC, 1607), per-monitor DPI
  calls (`GetDpiForWindow`, 1607), WinHTTP with TLS 1.2, the Universal CRT and the API sets the
  exe imports (Windows 8 and later). The installer's minimum is 10.0.17763 (1809), the oldest
  Windows 10 still under Microsoft's extended servicing (LTSC 2019) when this was written.
- **Font**: Segoe UI Variable is Windows 11's. The theme's default names it; on Windows 10 the
  app uses Segoe UI instead (`platform::system::font_here`, checking the font's registration).
- **Snap layouts** (hovering maximize) are Windows 11's; on Windows 10 the button maximizes as
  before. Windows 10 draws square window corners; Windows 11 rounds them.
- **Default apps page**: the per-app deep link needs Windows 11 (see above); Windows 10 opens
  the Default apps page.
- Not verified: the app-drawn caption on Windows 10's frame (Windows 10 has a 1 px top border
  of its own that some apps with custom captions redraw), DPI changes across monitors, and
  every installer page on Windows 10.

### Linux AppImage

Built on **Ubuntu 22.04** (an older baseline than the 24.04 used for development), so it needs
**glibc 2.35** or newer and a libstdc++ with `GLIBCXX_3.4.29` (GCC 11's; the release prints the
newest `GLIBC_` and `GLIBCXX_` symbol versions the binary uses). It also needs
**libpipewire-0.3 0.3.50 or newer** from the system: of the 24 PipeWire functions the binary
imports (the release lists them), the newest is `pw_stream_get_time_n` (0.3.50); every one is in
0.3.50's library (checked against one built from source). Building needs more: the PipeWire
bindings (libspa 0.10) use SPA headers of 0.3.65 (inline functions and a struct field), and
Ubuntu 22.04 has 0.3.48, so the build uses PipeWire 0.3.65 built for the purpose
(`packaging/linux/pipewire-sdk.sh`, in a folder of its own; used for building and for the tests,
never shipped). On a system with an older or no libpipewire (Ubuntu 22.04, Linux Mint 21) the
AppImage's launcher says so in a dialog (kdialog, zenity or a notification) instead of a loader
error. Runs on: Ubuntu 22.10 and later, Debian 12 and later, Fedora 36 and later, Linux Mint 22,
Pop!_OS 22.04 (its PipeWire is newer), openSUSE Tumbleweed, Arch, and the like. Tested at run
time only against PipeWire 1.0.5 (Ubuntu 24.04: [Tested in WSL](linux.md#tested-in-wsl-and-what-was-not)).

What it carries, and why only that: `usr/bin/MarqueePlayer` (stripped; RUNPATH
`$ORIGIN/../lib`), `usr/lib/` with libxkbcommon, libxkbcommon-x11 and libxcb-xkb (small, not on
every X11 system, and nothing else depends on them in the process), DejaVu Sans as a fallback
font with `usr/share/marquee-player/fonts.conf` (the system's fontconfig configuration first,
then the AppImage's fonts, found relative to the file; `AppRun` sets `FONTCONFIG_FILE` unless it
is set), the desktop file, the icons in every hicolor size, the metainfo, and the README,
licence and notices in `usr/share/doc/marquee-player/`. From the system: glibc and libstdc++
(bundling libstdc++ breaks GPU drivers built with a newer one), libpipewire and libasound (they
load the sound server's own modules), fontconfig and freetype (always there, and an older
bundled fontconfig can misread the system's configuration and cache), the Wayland and X11 client
libraries (Mesa's EGL and GLX use the same ones, and a bundled older libwayland-client is known
to break newer Mesa), libEGL, libGL, Vulkan and the GPU driver, and the D-Bus session bus (zbus
talks to it directly). This follows the AppImage project's exclusion list, and the release
refuses a binary that links anything else. `AppRun` changes no library path, so programs the app
starts (the file manager, the browser) see the system as it is.

Desktop integration is the user's choice: the AppImage installs nothing on its first run (no
menu entry, no MIME registration); AppImageLauncher, Gear Lever or the desktop's own tools add
it to the menu, and its desktop file declares the formats for Open With. It runs with FUSE
(`chmod +x`, then run it) or without (`--appimage-extract-and-run`); the runtime is the static
type-2 one, which needs no libfuse2 on the system.

**Portable on Linux.** An AppImage is read-only, so the Windows marker has no counterpart
inside it; the AppImage convention is a folder beside it: `MarqueePlayer-<v>-x86_64.AppImage.home`
(make it, or run the AppImage once with `--appimage-portable-home`). With it the app keeps
everything there, in the XDG layout under it (`.config/marquee-player`, `.local/share/...`,
`.local/state/...`, `.cache/...`), whatever the session's `XDG_*` variables say; a `.config`
folder beside it (`--appimage-portable-config`) holds the settings instead. Its library folders
follow the drive's mount point as on Windows (a stick mounted under another user's
`/media/<user>/`). Renaming the AppImage (a new version) means renaming the folder with it.

The packaging inputs check out on Ubuntu 24.04 too (`desktop-file-validate` passes the desktop
file), but AppStream 1.0's `appstreamcli validate` fails the metainfo on a warning alone, and it
has one: `url-homepage-missing` (there is no homepage to name). Ubuntu 22.04, where releases
are built, has AppStream 0.15, which was not tried for this; a release run on a newer
system stops at that step until the metainfo has a `<url type="homepage">`.

### macOS disk image

`MarqueePlayer-<v>-macos-arm64.dmg`: open it and drag Marquee Player onto Applications.
Apple Silicon, macOS 11 or newer; signed ad hoc and not notarised, so the first launch needs
the user's leave once ([macOS > First launch](macos.md#first-launch)). What the bundle holds,
how it is signed and what the release checks: [macOS > The package](macos.md#the-package).

## Exe size

The release exe is about 44 MB (43,878,912 bytes in an earlier measurement, without the bench
harness; 38,089,728 after the size work below, before tag editing, smart playlists, the equalizer
and crossfade, the visualizer's styles and the editions). Those five features added about 5.8 MB:
their UI (in `crates/ui`, already at opt-level "z"), their Rust in the app and library crates (at 3,
for the reason below), lofty's writing and image re-encoding for the tag editor. A linker map of the
fat-LTO build does not tell crate from crate well enough to say more (most of the app's code ends
up as unnamed local functions and constants); a PDB-based tool (`cargo bloat`, `llvm-pdbutil`) would.
Tried then and not kept: `image-webp` at "z" (no change: its code is instantiated in `image`, at 3).
The one lever left with no cost to the hot paths is moving cold UI glue (View metadata and the tag
editor, the smart playlist editor, Settings > Equalizer and > Visualizer) out of the app crate into
a crate built for size, as `crates/ui` was: perhaps 1 to 2 MB, a refactor not done yet.

Measured from the linker map and the PDB of release 2026.11.15.1 (43.8 MB, before the profile
below), the bytes went to:

| What | MB |
| --- | ---: |
| The generated UI (`ui/*.slint` as Rust, with Slint's generics instantiated for it) | 11.1 (6.3 at opt-level "z") |
| Skia, prebuilt C++ (SPIR-V Cross for Direct3D 1.0 of it) | 6.1 |
| wgpu and naga (Direct3D 12 only; naga's WGSL front end is forced on by wgpu) | 3.5 |
| ICU word and line break dictionaries (Chinese/Japanese 2.0, Thai/Lao/Khmer/Burmese 1.0), pulled in by Slint's parley `complex-scripts` | 3.0 |
| The app's own code (with the bench harness, which releases now leave out: see below) | 2.9 |
| Other read-only data: strings, tables, vtables | 2.2 |
| Text layout and shaping (parley, harfrust, skrifa, swash, ICU code) | 2.1 |
| Slint runtime | 2.0 |
| SQLite 1.7, image codecs 1.5, audio with libopus 1.5, SVG 1.3, windowing 0.8, tags 0.7, serde/TOML 0.7, std 0.7, software renderer 0.6, icon 0.4, Skia's Rust side 0.4, padding, relocations and the rest 0.6 | 10.9 |

Since then the bench and measurement harness is the `bench` feature, off in releases
([Build and run](building.md#build-and-run)): measured on the `dist` profile with the release path remapping, the exe is
37,138,432 bytes without it and 38,719,488 with it, 1.5 MiB (4.1%) less.

Unwind and exception tables (`panic = "unwind"`) are 4.8 MB of these totals. The profile in
`Cargo.toml` optimizes the UI crate and a few crates that never run while scrolling,
playing or scanning for size, and keeps everything else, the app crate included, at
opt-level 3 (its comments say why). Checked against the frame-timing benches (`grid`,
`sidebar`, `playlist` with `bench-opengl`), `visualizer`, `covers` and a fresh scan: no
difference. The linker already applies `/OPT:REF,ICF` (rustc passes it).

The equalizer's copy and paste (the other filter kinds, the format readers, the GraphicEQ fit, the
preview's UI) took the release exe (`cargo build --release -p marquee-player`, no bench harness)
from 44,274,176 to 44,966,912 bytes: 0.69 MB, 1.6% (not broken down by crate). `player-core` keeps
full optimisation.
