# Linux

Linux is a first-class build beside Windows: one source tree, the system's own parts behind
the platform seam (`crates/app/src/platform/`, one module per system; `player_core::output`
for sound; `library::dirwatch` for folder watching; `crates/http`). Windows is unchanged by it.

## Building

On Ubuntu 24.04 (Debian names; other distributions have the same libraries under their own
names):

    sudo apt install build-essential cmake pkg-config clang libclang-dev \
        libpipewire-0.3-dev libspa-0.2-dev libasound2-dev \
        libfontconfig1-dev libfreetype-dev libxkbcommon-dev libwayland-dev \
        libx11-dev libxcursor-dev libxrandr-dev libxi-dev libgl1-mesa-dev libegl1-mesa-dev
    rustup toolchain install 1.99.1      # the same toolchain as Windows
    cargo build -p marquee-player --release      # target/release/MarqueePlayer

`clang` and `libclang-dev` are for the PipeWire bindings (bindgen), `cmake` for libopus. The
first build downloads Skia's prebuilt binaries for Linux, as on Windows. Tests:
`cargo test --workspace --release`, with `ffmpeg` on PATH for the fixtures (ffmpeg 8 or newer
for the MP4 gapless check; older ones write that fixture one frame off and the test says it
skipped it), in any target folder: the tag editor's tests make and write their fixtures under
the system's temporary folder, the only place its write guard lets them write. `cargo clippy --workspace
--all-targets -- -D warnings` is clean on Linux too, with and without the bench harness.

`packaging/linux/` has what a package needs: the desktop file and AppStream metainfo
(`app.marqueeplayer.MarqueePlayer.*`), the icon in the hicolor sizes (`make-icons.sh` renders
them from `tray/icon_art.rs`, the artwork of the Windows icon) and `FIRST-TEST.md`, the checklist for
a first run on a real desktop. `headless-session.sh` starts a private headless session (D-Bus,
PipeWire with two null sinks, headless Weston and Xvfb, software rendering) to run the app and
its benches without a desktop, in WSL too without WSLg (see [Tested in WSL](#tested-in-wsl-and-what-was-not) below).

## Running, and what it needs

The binary needs from the system: glibc (the version of the system it was built on, or newer:
build on the oldest distribution it should run on), libstdc++ (Skia), libfontconfig and
libfreetype, libpipewire-0.3 (0.3.50 or newer; on every desktop distribution of the last years,
also those that play through PulseAudio) and libasound; at run time libwayland-client and
libxkbcommon (Wayland) or libX11 and its friends (X11), libEGL and libGL from the GPU driver
(Mesa's llvmpipe without one), and a D-Bus session bus for the media controls, the tray and
"Show in file manager" (each is simply absent without one). An AppImage should bundle none of
the graphics, sound or D-Bus libraries (they must match the system's driver and servers) and
may bundle the rest; libpipewire above all must be the system's, as it loads the server's own
modules.

## What is different on Linux

- **Sound: PipeWire, natively** (`player-core/src/output/pipewire.rs`), a second
  implementation of the engine's `Device`. One thread owns every PipeWire object; the stream is
  pulled by the graph (a real-time callback copying from a lock-free ring: no allocation or
  locking in it), the ring itself kept filled to 50 ms or three graph cycles whatever the graph
  holds beyond it (`output/ring.rs`), positions and the visualizer's tap counting what the graph
  has not played yet. Underruns are logged as on macOS. Gapless across tracks of a rate; a rate change
  reopens the stream (PipeWire switches the graph to the track's rate when its configuration
  allows and nothing else holds it, else resamples). Pause deactivates the stream (the graph
  stops calling it: nothing runs while paused); a seek, play-now or stop is a cut the callback
  does itself, fading out the very frames that follow what the graph already has (5 ms), so it
  is click-free whatever the graph's buffering. The app applies its volume, as on Windows; the
  desktop's mixer shows the stream (with the app's name and icon) and its own volume, as
  Windows' volume mixer does. **System default** leaves the stream to the session manager
  (WirePlumber), which follows the default device without a gap; a chosen device is the
  stream's target, and when it goes the stream moves to the default (the pill says so) and
  back when it returns. Settings > Output, the equalizer (whose per-device settings follow the
  device that plays) and the technical details name the sink the stream is really linked to,
  read from PipeWire's links: the default in effect (the "default" metadata's
  `default.audio.sink`, which WirePlumber keeps; not `default.configured.audio.sink`, which may
  name a device that is not there), or another one the session manager put it on (WirePlumber
  restores the device the user once moved the app to in the desktop's mixer). Sinks coming and
  going and the default changing are followed as they happen, also while paused or stopped and
  before anything played (a Bluetooth profile switch is a sink going and another coming; a
  virtual or filter sink, such as an equalizer's or a combined output, is a sink like any
  other, `Audio/Sink`, `Audio/Sink/Virtual`, or a pro-audio profile's `Audio/Duplex`). At
  start the default is read before the first list is made or the first stream opened (it comes
  a round trip after the sinks: before, a list or a stream that early took the first sink for
  the default, which is how the equalizer could name another device than the desktop's
  default).
- **Bit-perfect playback** stands for Windows' "Exclusive mode": PipeWire has no exclusive
  access to give, so the honest counterpart is asking the graph to run at the track's rate
  (`node.force-rate`), also while other programs play (they are resampled instead). The
  technical details' line says what happened: `Output: PipeWire, 96 kHz, bit-perfect rate, not
  resampled`, or `Output: PipeWire, 48 kHz (PipeWire resamples from 96 kHz)` with a note when
  PipeWire would not switch. Every stream asks for its rate (`node.rate`) anyway, so on an idle
  system the graph follows the music without the setting.
- **Without PipeWire** (an older system with PulseAudio, or bare ALSA) the app plays through
  ALSA's `default` device (`output/alsa.rs`): PulseAudio's ALSA plugin, or the hardware.
  `MP_AUDIO=alsa` forces it. There is no device list then (the sound server's or the system's
  default plays), and the Output line says `ALSA`.
- **The window** has no title bar from the desktop: the app draws its own caption, as on
  Windows, with client-side decorations on Wayland (xdg-shell) and X11 alike
  (`platform/linux/window.rs`). Drag the top strip to move (the compositor's own move: snapping
  and tiling work), double-click it to maximize, right-click it for the compositor's window
  menu (Wayland; X11 has no standard one), resize from any edge or corner. Server-side
  decorations are not used where the desktop prefers them (KDE): the window is `no-frame`
  (`Chrome.frameless`, set before it is shown), so it asks for client-side mode
  (xdg-decoration on Wayland, `_MOTIF_WM_HINTS` on X11) and the app's look runs to the top
  edge on every desktop. (Slint applies a window's properties again whenever one changes, the
  title with every track: without `no-frame` it asked for the system's decorations right after
  the window was made without them, and KWin drew its title bar over the app's.) Compositors
  draw no shadow round a window that draws its own frame (KWin, GNOME's), so a hairline in the
  theme's border colour marks the window's edge while it is not maximized. The window's size and
  maximized state come back on the next start; its position only on X11 (Wayland does not let a
  program place its window).
- **The tray** is a StatusNotifierItem of the app's own (`platform/linux/tray.rs`, through
  ksni, the crate and features Slint's tray uses; Slint's item answers only `Activate`): KDE
  shows it, GNOME with an AppIndicator extension, most other panels too, with the same menu,
  tooltip and animation as on Windows. `Activate` is the click, `SecondaryActivate` (the
  middle button on KDE) the next track, `Scroll` the volume (see [Tray icon](features/navigation.md#tray-icon)); a host sends
  no double click, so two activations within 400 ms are one. `Scroll`'s delta differs by
  host: KDE sends Qt's angle delta (120 a notch, up positive; a touchpad or a fine wheel a few
  units an event), GNOME's AppIndicator extension `Math.floor` of Clutter's smooth delta (1 a
  notch, down positive: `scroll(dx, dy)` in its `appIndicator.js`). The desktop
  (`XDG_CURRENT_DESKTOP`) says which; on another one a delta under 30 is taken as GNOME's.
  (The extension's rounding turns a touchpad's fractions into 0 going down and -1 going up, so
  there a touchpad turns the volume up faster than down; nothing the item can undo.) The item
  is made even when no panel is there yet, and registers as soon as one comes (an app started
  with the session before its panel, a panel restarted, an extension turned on later). Its id is
  `marquee-player` (KDE's System Tray settings list it under that). On a
  desktop without a tray host, closing the window quits (Settings > Window > Close button then
  says so), so the app is never left running with no way back. The `webbrowser` crate in the
  lock file is not the app's: Slint's winit backend links it for its `open_url` (a few kilobytes).
- **Media controls: MPRIS** (`platform/linux/media.rs`, over zbus): the Plasma media applet
  and lock screen, GNOME's controls, `playerctl`, the media keys. It announces the track, its
  cover, the position, shuffle, repeat and volume, and takes play, pause, next, previous, seek
  and set position, shuffle, repeat, volume, raise and quit. Bus name
  `org.mpris.MediaPlayer2.MarqueePlayer`, DesktopEntry `app.marqueeplayer.MarqueePlayer`.
- **One instance per data folder** through a Unix socket in `$XDG_RUNTIME_DIR`; a second
  launch hands over its files and asks for the window, as `WM_COPYDATA` does on Windows.
- **Files**: XDG directories (see [Files](architecture.md#files)); the folder watcher uses inotify (a watch per
  folder, folders that appear watched too; when `fs.inotify.max_user_watches` is too low for
  the library, the pill says so once with the command that raises it, and changes elsewhere
  are seen at the next start). Tag editing replaces a file with `rename(2)` after giving the
  checked copy the original's permission bits, owner (where allowed) and extended attributes;
  a file that is playing plays on (the kernel keeps the old one until it is closed). Its
  creation time cannot be kept (Linux has no call to set it), and a symbolic link is refused.
- **Show in file manager**: `org.freedesktop.FileManager1` (the file selected in Dolphin,
  Nautilus...), else `xdg-open` on its folder. Links open with `xdg-open`. Copy and the
  equalizer's Paste use the toolkit's clipboard (Wayland or X11), through a hidden multi-line
  text field (`clipboard-copy`, `clipboard-paste` in app.slint).
- **Files from outside**: on the command line and from a second launch, as on Windows; dropped
  from a file manager onto the window on X11 (they play; the drop point is not known, so a drop
  on the queue plays too). Wayland drops are not taken: winit 0.30, under Slint, has no drag
  and drop on Wayland.
- **Fonts**: the desktop's UI font (KDE's or GNOME's setting), else fontconfig's default; any
  script it lacks falls back through fontconfig (CJK needs a CJK font installed, such as Noto
  Sans CJK; without one, such titles show as boxes, nothing worse). `theme.toml`'s
  `font_family` is empty by default here (the desktop's font); a name there wins.
- **Rendering**: Skia, Slint trying OpenGL first, then Vulkan through wgpu, then Skia's
  software surface, at window creation; all three draw the live blur.
- **Words**: "Show in file manager", "Bit-perfect playback", "the system" where Windows names
  itself (`platform::Words`).
- **Middle-click scrolling, the remappable keys** (the pin keys taken before a text field sees
  them), the side buttons and accessibility (AT-SPI, through Slint's AccessKit) work as on
  Windows. There are no snap layouts (a Windows 11 feature) and no "Default apps": the desktop
  file declares the formats, and the desktop's own Open With and default applications use it.
  There is no "Start with Windows" counterpart; on Linux the desktop's Autostart settings start
  any app.

## Tested in WSL, and what was not

The Linux build is built and tested in WSL2 (Ubuntu 24.04) without WSLg: a private session with its
own D-Bus, PipeWire 1.0.5 and WirePlumber with two null sinks, headless Weston (Wayland) and
Xvfb (X11), Mesa's llvmpipe and lavapipe only. Slint forces X11 when it finds WSL
(`/proc/sys/fs/binfmt_misc/WSLInterop`, `/run/WSL`), so the Wayland runs hide those in a private
mount namespace (`packaging/linux/headless-session.sh` sets all this up). Installed in the WSL
distribution for it, besides the build packages above and rustup with 1.99.1: `ffmpeg`, `dbus`,
`dbus-user-session`, `pipewire`, `pipewire-bin`, `wireplumber`, `pipewire-alsa`, `weston`,
`xvfb`, `xauth`, `libgl1-mesa-dri`, `libegl1`, `libgles2`, `libvulkan1`, `mesa-vulkan-drivers`,
`libxkbcommon-x11-0`, `libx11-xcb1`, `fontconfig`, `fonts-dejavu-core`, `fonts-noto-cjk`,
`python3` (the recording checks), `strace`, `wayland-utils`, `x11-utils` (`xdotool` and
`libdbus-1-dev` are not needed), and for the decorations
`kwin-wayland`, `kwin-x11` and `kwin-style-breeze` (with `--no-install-recommends`: KWin and the
KDE Frameworks it needs, no Plasma desktop).

The same session builds, lints and tests the whole tree (the Navidrome library and
streaming, write-back, multi-select, search operators, lyrics, the tray, the 0600
credentials files and the folder watcher's re-arm among it) and runs the quick benches on
Wayland and X11. What was checked there beyond them:

- **Decorations on KWin**: a headless KWin (`kwin_wayland --virtual`, and `kwin_x11` on the
  session's Xvfb) in the private session. Wayland, with `WAYLAND_DEBUG=1`: the app binds
  `zxdg_decoration_manager_v1`, asks `set_mode(client_side)`, and KWin configures client-side
  (without the fix: `set_mode(server_side)`, configured server-side, KWin's title bar). X11: the
  window's `_MOTIF_WM_HINTS` say no decorations (without it: decorations on). Seen at protocol level,
  not on a screen: the move, resize and window-menu requests need a real pointer's serial, which
  the headless session has no way to give.
- **The tray**, through a stand-in StatusNotifierWatcher and host on the private bus (a Python
  script calling what a panel calls): the item registers (also when the watcher comes after
  it), its id, title, tooltip, 32 px icon and menu read back; `Activate` plays and pauses, two
  within 100 ms do not, `SecondaryActivate` skips, `Scroll` steps the volume by KDE's and by
  GNOME's deltas; MPRIS reported each change and its `Quit` closed the app.
- **The default device**: `MP_BENCH=output-default` (see [The bench runner](testing.md#the-bench-runner)), with
  `wpctl set-default` between the two null sinks, before playing, playing, paused and stopped.
- **Hover at whole and fractional scale**: `MP_BENCH=row-hover` at 1, 1.25 and 1.5
  (`SLINT_SCALE_FACTOR`), X11 and Wayland, and the tour, Settings, About and the phase 4 pages
  at 1.25 on Wayland.
- **X11's test window**: Xvfb has no window manager, so nothing maximized the window
  `MP_WINDOW=secondary` asks to be maximized, and on X11 the benches ran at 1440 x 900 where on
  Wayland they ran on the whole screen (the playlist bench's selection clicked a row under the
  player bar there). With no window manager the test window now fills its monitor itself
  (`platform/linux/devwindow.rs`; a size the bench sets is kept).

Checked the same way, with nothing else loading the machine:

- **The interface size on Linux**: `zoom` on Weston, on Xvfb with no window manager and with
  `kwin_x11` as its window manager, and on a headless KWin (`kwin_wayland --virtual --scale`)
  at a compositor scale of 1.25 and 1.5, where the app's factor is the compositor's times the
  zoom. Its keys go through the window's key dispatch, Ctrl and the wheel and the pinch's touches
  through the handlers winit's events reach (no real wheel, touchpad or touchscreen there). The
  window's size kept at 150% across two restarts on each (a settings file of 1600 x 1000, no
  `MP_WINDOW`): kept. Without the fix, Weston left the window at 1440 x 900 under its 200%
  minimum of 1904 x 1000.
- **The row's hover and the edge line at 70% and 200%** (`row-hover-z70`, `row-hover-z200`),
  on all of the above.
- **`viz-look` and `streaming`** three times each on Wayland: both pass every run on a quiet machine; each
  once failed two runs in three there (a dead band that kept the cover at 1.0003; a status
  read before it was the new track's).
- **ListenBrainz**: its tests, the `listenbrainz` bench on Wayland and X11, `listenbrainz.json`
  written 0600 (`platform::secret::write_private`), and the token page through the open-a-link
  path with a stand-in in place of `xdg-open` (a unit test; no browser).
- **The checks before a push** (`cargo xtask check`) as root on a
  checkout another user owns: trusted for the run, the whitespace step working (it caught a
  planted trailing space); on a copy with no `.git`: skipped, said so.

Seen and not fixed: under `kwin_x11` the window's position comes back as 0, 0 (KWin then
places it), where on X11 without a window manager it is kept; a real KDE X11 session should
show whether that is the headless KWin or the placement code.

Not verifiable there, and covered by `packaging/linux/FIRST-TEST.md`: real
sound hardware and its rates, Bluetooth and its profile switches, a real panel's tray (the
wheel's direction on a real KDE panel and GNOME's extension, a touchpad's small deltas), the
Plasma media applet, KWin's decorations, shadow, snapping and tiling on a screen, the window
menu, touch and touchpad pinches, a real wheel with Ctrl held, file manager drops, file
associations, HiDPI and several monitors (moving the window between monitors of different
scales with the interface size set), a real GPU's performance, and the
clipboard on Wayland:
the equalizer's Copy and Paste went through X11's real clipboard there (`MP_BENCH=eq-paste` with
`MP_BENCH_REAL_CLIPBOARD=1` under Xvfb: copied, read back, lines kept), but Wayland gives the
clipboard only to a window with the keyboard focus and a real input event, which the headless
Weston session has no way to give (the copy is accepted and never reaches the compositor).
