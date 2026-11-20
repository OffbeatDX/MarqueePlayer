# First test on a real Linux desktop

Ten minutes, KDE Plasma on Wayland first, then any other desktop you have (GNOME, X11). Each
step: what to do, what should happen, what to report if it does not. These are the things
that could not be checked in the headless test session the port was built in (no real sound
card, no panel, no file manager, no real GPU).

Before you start: a build of Marquee Player (`MarqueePlayer`, or the AppImage `cargo xtask release` makes in `dist/`), a few albums
in a folder (ideally one album that is gapless, a live album or a DJ mix, and one track at
96 kHz or 44.1 kHz besides the usual 48 kHz), headphones or speakers. For a clean test, run it
with a fresh data folder so your real one is not touched:

    MP_DATA_DIR=/tmp/mp-first-test ./MarqueePlayer

To report anything: the step's number, what happened instead, and the log
(`/tmp/mp-first-test/marquee-player.log` with the command above, otherwise
`~/.local/state/marquee-player/marquee-player.log`). Its first lines name the system, the
renderer and the sound output. Settings > About > Copy gives one line with the same.

1. **It starts, and looks right.** Run it. *Expected:* the window opens without the desktop's
   own title bar; the page runs to the top edge as on Windows; text is crisp; the fonts are your
   desktop's. *Report:* a title bar from the desktop above the app's own page, blurry or tiny
   text, or a crash (with the log).
2. **Add your music.** The button on the empty home page, then pick the folder. *Expected:* a
   folder dialog of your desktop (KDE's or GNOME's); the scan runs; covers appear. *Report:* no
   dialog, or an error.
3. **Sound through PipeWire.** Play an album. *Expected:* it plays on your usual output. In
   your desktop's volume mixer (KDE: the speaker icon in the panel, then the applications tab)
   "Marquee Player" appears with the app's icon, with its own volume slider. Settings > Window >
   Show technical details on; Now playing shows `Output: PipeWire, 48 kHz, not resampled` (or the
   rate your system runs at). *Report:* silence, crackles, the app missing from the mixer, or what
   the Output line says.
4. **Gapless.** Play the gapless album across a track change. *Expected:* no gap, no click
   between tracks. *Report:* a gap or a click, and at which track.
5. **Another sample rate.** Play the 96 kHz (or 44.1 kHz) track, then a 48 kHz one. *Expected:*
   both play; at the change of rate a short fade, not a click. The Output line says
   `Output: PipeWire, 48 kHz (PipeWire resamples from 96 kHz)` unless your PipeWire allows 96 kHz.
   Then Settings > Output > Bit-perfect playback on, and play the 96 kHz track again. *Expected:*
   either `Output: PipeWire, 96 kHz, bit-perfect rate, not resampled`, or a note saying PipeWire's
   allowed rates leave 96 kHz out (most distributions allow only 48 kHz until
   `default.clock.allowed-rates` is set). *Report:* what the line says each time, and any glitch.
6. **Seek, pause, the visualizer and lyrics.** Click in the seek bar, press Space twice, open Now
   playing with the visualizer on. *Expected:* seeking and pausing are immediate and click-free;
   the bars move in time with the music (not ahead of or behind it). *Report:* a delay you can
   hear or see, or clicks.
7. **Switching outputs.** With music playing, plug in or switch to other headphones (or choose
   another output in Settings > Output). *Expected:* playback moves to it within a fraction of a
   second and goes on from where it was. Choose a device in Settings > Output, unplug it while
   playing: *expected:* it goes on on the default and the pill says so; plug it back: it returns.
   *Report:* silence, a restart from the beginning, or a stuck "not connected".
8. **The window frame.** Drag the top strip: the window moves (and snaps to a screen edge as
   your desktop does). Double-click it: maximize and back. Right-click it: your desktop's window
   menu (Wayland; X11 has none). Move the pointer to any edge or corner: a resize cursor; drag it:
   the window resizes. Near the top right corner the three buttons show: minimize, maximize,
   close all work. *Report:* which of these does not, and on Wayland or X11.
9. **Tray icon.** *Expected:* the app's icon in the panel's system tray (KDE: maybe under the
   arrow; GNOME: only with the AppIndicator extension). While playing, its bars move. Left click:
   the window comes back. Right click: Play/Pause, Next, Previous, Show, About, Quit, all working.
   Hover: the playing track's title. Close the window: it hides; the tray brings it back.
   *Without a tray* (GNOME without the extension): closing the window quits, and Settings > Window
   > Close button says so. *Report:* no icon on KDE, a wrong or blurry icon, a menu entry that
   does nothing, or the window gone with no way back.
10. **Media keys and the media applet.** Press the keyboard's play/pause and next keys. Open
    the Plasma media applet (or the lock screen). *Expected:* the keys work; the applet shows the
    title, artist, album and cover, the app's name and icon; its buttons and seek bar work; the
    position moves; shuffle and repeat buttons (where the applet has them) follow the app.
    *Report:* what is missing or wrong (`playerctl metadata` shows what the app announces).
11. **One instance, and files from outside.** With the app running, run it again: *expected:*
    the window comes forward (or flashes in the taskbar, which some desktops do instead), no second
    window. Run it with an audio file or a folder as argument: it plays. Drag files from the file
    manager onto the window: on X11 they play; on Wayland drops are not taken yet (known).
    *Report:* a second window, or files ignored.
12. **Show in file manager.** Right-click a track, "Show in file manager". *Expected:* Dolphin
    (or Files) opens with the file selected. *Report:* nothing, or the wrong folder.
13. **File associations.** If installed with the desktop file (an AppImage integrated by
    AppImageLauncher, or the desktop file copied to `~/.local/share/applications`):
    right-click an `.flac` in the file manager, Open with: Marquee Player is offered, and opening
    plays it (in the running app). *Report:* not offered, or a new instance.
14. **HiDPI and fractional scaling.** With your display at 125%, 150% or 200% (KDE: Display
    Configuration > Scale): text and covers sharp, the window's size sensible, the resize edges
    and caption buttons where they look to be. *Report:* blur, wrong sizes, or edges/buttons that
    do not line up with what is drawn.
15. **Two monitors.** Move the window to the other monitor, maximize it there, quit, start
    again. *Expected:* same size; maximized again (Wayland places windows itself, so it may come
    back on the first monitor; X11 puts it back where it was). Move it between monitors of
    different scales: it redraws at the new scale. *Report:* what differs.
16. **Autostart.** The app has no setting of its own for it (neither on Windows); the desktop
    does it: KDE System Settings > Autostart > Add Application > Marquee Player (GNOME: Tweaks >
    Startup Applications). Log out and in. *Expected:* it starts, in the tray if it was closed to
    the tray... or with its window. *Report:* it does not start, or starts twice.
17. **Idle.** Pause, close the window to the tray, wait a minute, then `top -p $(pgrep MarqueePlayer)`.
    *Expected:* 0.0% CPU. *Report:* anything above a fraction of a percent.
18. **The equalizer's Copy and Paste (Wayland above all).** Settings > Equalizer, choose Bass
    boost, press **Copy**, then paste into a text editor. *Expected:* lines starting
    `# Marquee Player equalizer preset: Bass boost`, `Preamp: …`, ten `Filter n: ON PK …` lines,
    each on its own line. Then copy this from the editor and press **Paste** (or `Ctrl+V` on the
    equalizer page): `Preamp: -3 dB` and, on the next line, `Filter 1: ON PK Fc 100 Hz Gain 3 dB Q 1`.
    *Expected:* a preview "Equalizer APO text · 1 filter"; Cancel leaves the equalizer as it was.
    The headless test session could check this round trip on X11 only: Wayland gives the
    clipboard only to a window with the keyboard focus and a real key or click, which that
    session has no way to give. *Report:* nothing in the editor, the lines run together, or
    "Nothing to paste".
19. **The equalizer's device is the default.** Settings > Equalizer: the device it names (and
    "Separate setting for ...") is the one your desktop calls the default output (KDE: the
    speaker icon in the panel), before anything plays and while playing; Settings > Output says
    "Playing on" the same. Change the default in the panel (to headphones, a Bluetooth device, an
    equalizer's virtual sink if you have one), while playing, paused and stopped: *expected:* the
    sound moves and both pages name the new one within a second. Switch a Bluetooth headset's
    profile (A2DP and back): the same. *Report:* the two names, and what the panel says.
20. **Hovering a track's play button.** On an album page, rest the pointer on a track's number:
    the play button shows; hold the mouse still, then move it a hair. *Expected:* the row stays
    lit, nothing blinks, and a click there plays the track (it does not just select the row).
    Hover the row's queue, Play next and heart buttons: they stay visible. Try at 100% and at a
    fractional scale (125%, 150%). *Report:* blinking, or a click that only selects.
21. **No title bar from the desktop, and the window's edge (KDE above all).** On Plasma
    (Wayland, then X11): *expected:* no KWin title bar above the app's own top strip (step 1),
    and a thin light line round the window while it is not maximized (the desktop draws no
    shadow for it). Then step 8 again: move, snap to an edge and to a quarter (tiling), maximize
    by double-click and back, minimize, resize from each edge. *Report:* a title bar, a missing
    edge, or what does not work.
22. **The tray's wheel and middle button.** Over the tray icon: the wheel up raises the volume,
    down lowers it (5% a notch; the tooltip says so for a moment), with a mouse wheel and with a
    touchpad's two-finger scroll; the middle button skips to the next track; two quick clicks
    show or hide the window. On GNOME (AppIndicator extension) the same. *Report:* the direction,
    the step, and the desktop.
