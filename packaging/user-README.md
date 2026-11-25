# Marquee Player

A music player for Windows, Linux and macOS that plays the music on your own drives and puts the
album art first. No account, no streaming.

## Which download

| Download | What it is |
| --- | --- |
| `MarqueePlayer-<version>-setup.exe` | Windows, installed: in the Start menu, in Settings > Apps, under "Open with" for your music files. Your library is kept in your user folders. |
| `MarqueePlayer-<version>-portable.zip` | Windows, portable: a folder you can put anywhere, a USB stick included. Your library is kept in that folder, and goes where it goes. Nothing is installed. |
| `MarqueePlayer-<version>-x86_64.AppImage` | Linux: one file to run. |
| `MarqueePlayer-<version>-macos-arm64.dmg` | macOS on Apple Silicon (M1 and later): a disk image to drag the app from into Applications. |

## What you need

On Windows: Windows 11, or Windows 10 (version 1809 or newer), 64-bit. Nothing else: what the
program needs from Microsoft's Visual C++ runtime is in its own folder.

On Linux:

- A 64-bit desktop Linux from the last few years, on Wayland or X11: Debian 12, Ubuntu 22.10,
  Fedora 36, Linux Mint 22, Pop!_OS 22.04 or newer, Arch, openSUSE Tumbleweed, or the like. It
  needs PipeWire's library (libpipewire 0.3.50 or newer, which these systems have, also those
  that play sound through PulseAudio). Ubuntu 22.04 and Linux Mint 21 have an older one: there
  the AppImage says so and does not start. KDE Plasma shows the tray icon; GNOME shows it with
  the AppIndicator extension (without a tray, closing the window quits the app).
- To see Japanese, Chinese or Korean titles, a font for them (most systems have Noto Sans CJK).

On macOS: macOS 11 Big Sur or newer, on a Mac with Apple Silicon. Nothing else.

## "Windows protected your PC"

Marquee Player is not signed with a code-signing certificate (they cost money every year), so
the first time you run the installer or the program, Windows SmartScreen may show a blue
"Windows protected your PC" box. It says only that Windows does not know this file yet, not that
anything wrong was found in it. To go on: **More info**, then **Run anyway**. Do this only with a
file from the project's own releases page (the address Settings > About gives) or from someone you
trust. Your browser may also ask whether to keep the
download; choose **Keep**.

## "Apple could not verify..." (macOS)

Marquee Player for macOS is not signed with an Apple Developer ID nor notarised by Apple (that
takes a paid Apple account), so the first time you open it, macOS stops it and says it cannot
verify the app or its developer. That is all it means. To open it, once:

- **macOS 15 Sequoia and later**: try to open it, close the message, then open **System
  Settings > Privacy & Security**, scroll down to **Security**, click **Open Anyway** beside
  Marquee Player, and confirm.
- **macOS 11 to 14**: in Applications, right-click (or Control-click) Marquee Player, choose
  **Open**, then **Open** in the message.
- Or, in Terminal: `xattr -dr com.apple.quarantine /Applications/MarqueePlayer.app`

After that it opens normally. Do this only with a file from the project's own releases page.

## Getting started

**Installed (Windows).** Run `MarqueePlayer-<version>-setup.exe`. It asks whether to install
**for you only** (no administrator rights needed; the usual choice) or **for all users** of the
computer (asks for an administrator), then for a desktop shortcut. At the end you can start
Marquee Player, and open Windows' **Default apps** settings to make it the program that opens
your music files when you double-click them (Windows lets only you choose that, so setup
cannot). You can do it later too: **Settings > Window > Default app for music files**. Either
way, Marquee Player is under **Open with** (right-click a music file) for every format it plays.

**Portable (Windows).** Right-click the zip, **Extract All…**, and put the folder where you
like (for example `C:\Programs\Marquee Player`, or a USB stick). Run `MarqueePlayer.exe` in it.
Extract the whole zip first: run from inside the zip, the program is missing the files beside it.
The file `portable.txt` beside the program makes it keep everything in the `Data` folder next to
it: settings, your library with its plays and playlists, the caches. Copy the folder and the
library comes with it. Music on the same USB stick is found again when Windows gives the stick
another drive letter. The folder must be one you can write to (not a CD, not a read-only stick):
if it is not, Marquee Player says so and does not start. Delete `portable.txt` to have the
program keep its files in your user folders, like the installed one.

**Linux.** Make the AppImage executable (right-click it, Properties, "Is executable", or
`chmod +x MarqueePlayer-*.AppImage`) and run it. Nothing is installed: to have it in your
application menu and under Open With, use your desktop's tool for AppImages (Gear Lever,
AppImageLauncher), or keep starting the file. If your system has no FUSE, run it with
`--appimage-extract-and-run`. To make it portable (its library beside it, on a USB stick), make
a folder beside it with the same name plus `.home`
(`MarqueePlayer-<version>-x86_64.AppImage.home`), or run it once with
`--appimage-portable-home`; rename the folder with the file when you update.

**macOS.** Open the `.dmg` and drag **Marquee Player** onto **Applications**, then open it from
Applications (the first time, see "Apple could not verify..." above). Finder lists it under
**Open With** for your music files. The keys below that use **Ctrl** use **Command** (⌘)
instead, and **⌘Q** quits. Its icon in the menu bar (top right) is the tray icon: click it for
its menu. **Settings > Window > Open at login** starts it there when you log in.

Then, in any edition: add the folder your music is in: the button on the empty home page, or
**Settings > Library > Add folder**. Albums start to appear within seconds, folder by folder,
and you can play one while the rest is read. The first scan reads every file, so a large
library on a slow drive (a USB disk, a network share) takes a while: the foot of the sidebar,
the top of the home page and **Settings > Library** say how far it is, about how long is left,
and when the drive is slow or has stopped answering. You can quit, or press **Stop** in
Settings, at any time: what has been read stays, and the next start (or **Resume**) reads only
the rest. Later starts only look at what changed.

**Start with Windows** (**Settings > Window**, off unless you turn it on) starts Marquee Player
in the tray when you sign in, ready for the media keys. In the portable edition it puts the
program's location in Windows' startup list, the only thing it writes outside its folder: turn
it off before you move the folder or unplug the stick.

Your music folders are only read, unless you turn on tag editing (below). Marquee Player
never moves, renames or deletes your music files, and writes nothing else into those
folders (unless you choose to save an exported playlist there).

Formats: FLAC, MP3, AAC and ALAC (`.m4a`), Opus, Ogg Vorbis, WAV, AIFF, MP2. AAC with more
than two channels (5.1) cannot always be played (it depends on how the file was made), nor can
HE-AAC: such a file is skipped, and the app says that the format is the reason, not your file.

Albums are put together from your files' tags. Tracks with several artists or genres show up
under each; a compilation without an album artist is one album under "Various Artists"; a
missing title or track number is taken from the file name. If an album looks wrong, right-click
it (or a track) and choose **View metadata**: it says where every value came from. Fixing the
tags is the cure: in Marquee Player itself (below), or in a tag editor (Mp3tag, MusicBrainz
Picard), letting the app rescan.

## Fixing tags (off until you turn it on)

**Settings > Library > Edit tags** lets **View metadata** change your files: their tags (title,
artists, album, album artist, numbers, dates, genres, label, comment and more) and their front
cover. Off, the app only reads your music. On:

- Press **Enter** or **F2** on a field, or click its value, to edit it; **Enter** keeps the
  change, **Esc** cancels, **Tab** goes to the next field. A field with several values (artists,
  genres) has a box per value: **Ctrl+Enter** adds one, **Alt+↑/↓** moves one.
- An album's view edits the album's fields once for all its tracks, and its track list in place
  (number, title, artist). Where the tracks disagree (an album artist missing on one, numbers
  with gaps) it offers the obvious fix.
- Nothing is written until you **Review and save** (**Ctrl+S**): it lists every file and every
  change, old value to new. Closing with unsaved changes asks first, and so does quitting (from the
  tray icon too).
- Each file is written as a copy beside it and checked (its other tags, its pictures and its
  audio unchanged) before it replaces the file. A file that does not check out, or that is
  read-only, in use by another program or changed since you started editing, is left as it was,
  and the app says which and why. A track that is playing can be saved; it plays on.
- **Edit history** at the bottom of View metadata undoes a save (it refuses a file that changed
  since). The last 100 saves are kept, at most 256 MB with the covers they replaced;
  **Settings > Library > Undo history** shows how much and can clear it.

Raw AAC files (`.aac`) cannot be edited, nor MP3 files with the old ID3v2.2 tag; View metadata
says why. As with any tool that writes to your music: keep a backup of your music.

## Smart playlists

A smart playlist fills itself: you say what belongs in it, and it always holds the tracks that
match now. **Playlists > New smart playlist…** opens the editor, where each rule reads as a
sentence: "Genre is Jazz", "Year is at least 1990", "Last played is not in the last 6 months",
"Plays in the last 30 days is at least 3". Pick a field, a condition and a value (artists,
genres, labels and formats are chosen from what your library holds; type a few letters to find
one). Tracks can match **all** the rules or **any** of them, and **Add group** mixes the two
("lossless, and a favourite or played often"). Below the rules: the order, and an optional limit
("25 tracks", "2 hours", chosen by most played, most recently added or at random). The count and
a preview update as you change the rules. To start quickly, pick one of the starting points
("Recently added and unplayed", "Most played this year", "Lossless only"...) and change it.

Smart playlists sit beside your other playlists (marked with a filter icon), in the sidebar, on
the Playlists page with their rules in words, and as home shelves. Their page has **Edit rules**,
**Reshuffle** for a random one, and under **⋯** Rename, Duplicate, **Save as ordinary playlist**
(a fixed copy of what it holds now) and Export. A smart playlist keeps up with your library: a
new scan, a favourite or a play changes what is on its page. The queue you started
from it does not change under you. You cannot drag tracks into one or remove tracks from it:
change its rules instead.

## Equalizer and crossfade (both off until you turn them on)

**Settings > Equalizer** (or the equalizer button in the player bar, lit while it is on) shapes
the sound. Ten sliders, one per octave from 31 Hz to 16 kHz, under a curve that shows exactly
what is applied; with the keyboard, **Tab** to the sliders, **←/→** choose one, **↑/↓** move it
half a decibel. Presets: Flat, Bass boost and reducer, Treble boost and reducer, Vocal,
Loudness (for listening quietly), Warm, Bright, Spoken word. Choosing a preset or moving a
slider turns the equalizer on. **Parametric** shows each band in full to make your own: a
peak, a low or high shelf, a low-pass, high-pass, band-pass, notch or all-pass filter, each
with its frequency, gain (where it has one) and width; up to 32 bands. Showing a setting the
ten sliders cannot show changes nothing until you move a slider (a line under them says so).

- **Copy and Paste:** **Copy** puts the equalizer on the clipboard as Equalizer APO text (what
  Equalizer APO, Peace and AutoEq use), ready to send to a friend or use there; the arrow
  beside it copies it for Wavelet (GraphicEQ) or EasyEffects instead. **Paste** takes a setting
  from the clipboard: Equalizer APO or Peace text, an AutoEq file's contents, Room EQ Wizard's
  filter export, an EasyEffects preset. **Ctrl+C** and **Ctrl+V** do the same on the equalizer
  page. Nothing changes before you see what it would do: the curve over your current one, its
  filters, and anything that could not be played exactly as written, with why; **Apply** keeps
  it as a preset and plays it, **Cancel** or **Esc** leaves everything as it was.
- **The curve stays in view:** on a wide window it sits beside the settings; on a narrow one
  it stays at the top while the bands scroll under it.
- **Headphone corrections:** AutoEq (autoeq.app, or the AutoEq project on GitHub) has
  corrections for thousands of headphones. Copy the *ParametricEQ* text for yours and Paste it,
  or download the file and use the preset button's menu, **Import a preset file…** (it shows
  the same preview). A *GraphicEQ* curve works too: it is matched with bands, and the preview
  says how closely. Presets can be exported as Equalizer APO text or as a Marquee Player
  preset file.
- **Headphones and speakers:** with something playing, **Separate setting for <your
  device>** keeps a setting for that device alone; it comes back whenever that device plays.
- **No clipping:** the preamp (Auto) lowers the level by the largest boost, so boosting
  never pushes music past full scale. That is why a bass boost also sounds a little quieter
  overall.
- With the equalizer off, music plays exactly as before, bit for bit.

**Settings > Playback > Crossfade** fades each track into the next (1 to 12 seconds). An
album played in its order stays gapless, so live albums and mixes run on as recorded
(**Keep albums gapless**, on). Skipping to another track crossfades briefly, for half a
second. Tracks at different sample rates overlap too (an Opus track, always 48 kHz, into a
CD-quality FLAC at 44.1 kHz, say): the sound device cannot change rate in the middle of the
music, so the incoming track is converted to the rate playing, by a converter whose errors
are far below what 24-bit audio can hold. It stays converted until you pause, seek or stop,
after which it plays at its own rate again; with technical details shown, the line under
the track says so ("Converted from 44.1 kHz since the crossfade"). With crossfade off,
nothing is ever converted.

## The visualizer

Now playing shows the music moving: by default faint bars in the cover's colour along the
bottom of the page, with their reflection. The button at the top right of Now playing (or a
right-click on the page) turns it on and off and chooses its opacity, a preset and
where it shows: **behind the page**, as a **Visualizer** tab beside Up next and Lyrics, or the
**full page** with the cover and title in the corner. **Shift+V** moves it from one to the
next (and turns it on if it was off). **Settings > Visualizer** has everything, with a preview
that stays in view while you scroll: eight presets (Classic, Prism, Skyline, Calm, Bonfire, and three
around the cover: Halo, Saturn, Eclipse), four styles to build your own from
(bars; a line; the **ring** around the cover, in three kinds: Ripples, Saturn
and Eclipse, with a switch for the cover itself to pulse a little with the beat; and the
bonfire: sparks rising from a glowing line along the foot of the page), colours (the cover's own, classic meter
colours, rainbows, or three of your choice), the reflection, how fast it moves, how sensitive
it is, its **opacity** (behind the page and elsewhere each keep their own), and 30 frames a
second to save power on a laptop. **Reset** puts the defaults back (your own presets stay). It
only runs while you can see it and music plays.

**Your own presets:** the **Presets** button saves the look under a name, and renames,
updates or deletes it later; your presets come after the built-in ones, in Settings, in the
menu at the top right of Now playing, and with `C`. To share one, **Copy this look as text**
puts a single line on the clipboard to paste into a message (it starts `MarqueeViz/1`), and
**Paste a preset** (or `Ctrl+V` on that page) takes one from the clipboard, even inside a
longer message; **Export** and **Import** do the same with a file. A preset someone sent you
plays in the preview first and is added only when you say so (**Add preset**, or **Replace**
/ **Keep both** if you already have one of that name). A preset is only the look: not where
it shows, nor whether it is on.

Closing the window keeps the music playing from the tray icon (right-click it to quit; on a
Linux desktop without a tray, closing quits; on macOS, the menu-bar icon's menu or ⌘Q quits,
and the Dock icon brings the window back).
**Settings > Window > Close button** can make it quit instead.

## Keys worth knowing

| Key | Does |
| --- | --- |
| `Space` | Play / pause |
| `←` / `→` | Back / forward 10 seconds |
| `Ctrl+←` / `Ctrl+→` | Previous / next track |
| `Ctrl+↑` / `Ctrl+↓` | Volume up / down |
| `M` | Mute |
| `N` | Now playing |
| `V` / `C` | Visualizer on and off / its next preset |
| `Shift+V` | Where the visualizer shows: behind the page, in the panel, full page |
| `/` or `Ctrl+F` | Search |
| `Alt+←` / `Alt+→` | Back / forward between pages |
| `Ctrl+C` / `Ctrl+V` | On the equalizer page: copy it / paste a setting; on the visualizer page: its look as text / a preset |
| `Esc` | Close a panel, or go back |

**Settings > Keys** lists every key and lets you change them. Right-click an album or a
track (or press the Menu key) for queue and playlist actions.

## Online features (both off until you turn them on)

Marquee Player works entirely offline. Two optional features use the internet, and only
when you turn them on:

- **Lyrics lookup** (**Settings > Lyrics > Look up lyrics online**). For a
  track with no lyrics of its own (no `.lrc` file beside it, none in its tags), it sends
  the track's artist, title, album and length to LRCLIB (lrclib.net), a few seconds after
  the track starts. Answers are kept on your PC, so each track is asked about once.
- **Last.fm scrobbling** (**Settings > Connections > Last.fm**). Uses a Last.fm API account
  of your own: you paste its key and secret and allow access in your browser. It then sends
  each play to your Last.fm profile: artist, title, album, album artist, length, track
  number, the time it started and, when the file has one, its MusicBrainz track id.
- **Discord** (**Settings > Connections > Show on Discord**). Shows the track you are
  playing on your Discord profile, as "Listening to Marquee Player" (or under another
  name: paste the ID of a Discord application of your own, created at
  discord.com/developers/applications, into Application ID). Marquee Player then
  tells the Discord app running on your computer the title and artist of the playing track
  and how far in it is, and Discord shows it to whoever can see your activity there. It is
  cleared when you pause, stop or close Marquee Player.

Nothing else is sent anywhere.

## Where your data is kept

| What | Installed (Windows) | Portable (Windows) |
| --- | --- | --- |
| Settings, theme, the library database (plays, favourites, playlists), Last.fm connection, the log (and the previous run's, `marquee-player.previous.log`), the undo history of tag edits (`edits`) | `%APPDATA%\MarqueePlayer` | `Data` beside `MarqueePlayer.exe` |
| Caches: cover art, lyrics found online (safe to delete; made again as needed) | `%LOCALAPPDATA%\MarqueePlayer` | `Data\cache` |

On Linux the same things are in `~/.config/marquee-player` (settings, theme),
`~/.local/share/marquee-player` (the library database, Last.fm, the undo history of tag
edits), `~/.local/state/marquee-player` (the log) and `~/.cache/marquee-player` (the caches),
or in the same folders inside the `.home` folder beside a portable AppImage. On macOS they are
in `~/Library/Application Support/MarqueePlayer`, the caches in `~/Library/Caches/MarqueePlayer`.

**Settings > About** says which edition you are running, and shows each of these with its size
and an **Open folder** button.

- **To move to another PC:** quit Marquee Player (tray icon, Quit). Installed: copy the
  `%APPDATA%\MarqueePlayer` folder to the same place on the other PC (on Linux, the first three
  folders above), and keep your music at the same paths there (the library knows tracks by where
  their files are). Portable: copy the whole folder.
- **To start afresh:** quit Marquee Player, then rename or delete `%APPDATA%\MarqueePlayer`
  (and, if you like, `%LOCALAPPDATA%\MarqueePlayer`), or the portable folder's `Data`. The next
  start is a first start. Renaming keeps your plays and playlists to go back to.

## Updating, and going back

**To update**, quit Marquee Player (tray icon, Quit), then: installed, run the new
`setup.exe` (it updates in place, for the same users, and closes Marquee Player for you if it is
still running); portable, extract the new zip over the old folder (its `Data` stays); Linux,
replace the AppImage (and rename a `.home` folder with it); macOS, drag the new version into
Applications and replace the old one. Your settings and library stay where
they are. If the new version changes the library database's format, it updates the database on
its first start and first keeps a copy of the old one beside it, as `library.v<number>.bak.db`.

**To go back** to an earlier version, install or extract it again (or put the earlier AppImage
back). If the newer version had updated the database, the earlier one cannot open it and says
so when it starts. Then, with Marquee Player quit: in `%APPDATA%\MarqueePlayer` (or `Data`),
rename `library.db` to something else (for example `library.newer.db`), and rename the backup
copy (`library.v<number>.bak.db`) to `library.db`. Plays, favourites and playlist changes made
since the update are not in that copy.

## Uninstalling

**Installed:** Windows' **Settings > Apps > Installed apps**, Marquee Player, **Uninstall**. It
removes the program, its shortcuts and everything it registered with Windows, and then asks
whether to delete your library and settings too: choose **No** (the default) to keep your plays,
playlists and favourites for a later installation. Your music files are never touched.
**Portable:** quit it, turn off Start with Windows if you had turned it on, and delete the
folder (its `Data` is your library). **Linux:** delete the AppImage (and its `.home` folder, or
the folders listed above). **macOS:** turn off Open at login if you had turned it on, then move
Marquee Player from Applications to the Trash (and, to remove your library too, the two folders
under `~/Library` listed above).

## If something goes wrong

**To ask for help**, open **Settings > About** and press **Copy diagnostics** (under "Report a
problem"), then paste it into your report: an issue on the project's page (the address Settings >
About gives, under the copyright line). It is a short text:
the version, your system, the sound device, the size of your library, a few settings and the end
of the program's log, with your user name and home folder taken out. Read it first if you like;
Marquee Player sends nothing anywhere itself.

**If Marquee Player closes with a message** ("Something went wrong inside Marquee Player"), your
library, playlists and settings are kept. Start it again and use Copy diagnostics: the log of the
run that went wrong is kept beside the new one (`marquee-player.previous.log`, in the folder
Settings > About shows for "Log"), and the diagnostics include it.

**No sound?** If the sound device cannot be opened (nothing plugged in, a device another program
holds exclusively, one that was just removed), the player bar keeps the track and a message says
"No sound: the output device cannot be opened"; it plays on by itself as soon as a device is
available. **Settings > Output** shows what is playing where, and lets you choose a device.

**A song stops early or is skipped** with "Can't play" or "stopped early: the rest of its file
could not be read": the file is damaged or incomplete (an interrupted download or copy). Other
players will stop at the same place. Some AAC files with 5.1 sound are the exception: they are fine,
Marquee Player just cannot play that format (the message says so).

**It does not start, and says why:**
- "cannot write to the folder where it keeps your library": the data folder (see "Where your data
  is kept") is read-only or the disk is full.
- "The library database is damaged": choose **Yes** to start with a new library (your music files
  are not touched, and the damaged file is kept beside the new one, renamed); your plays, favourites
  and playlists then start afresh. If a copy from before the last update is mentioned
  (`library.v<number>.bak.db`), "Updating, and going back" says how to go back to it.
- "last opened by a newer version": you started an older version after a newer one; see
  "Updating, and going back".
- "Another program is using the library database": another copy of Marquee Player using the same
  folder, or a backup or sync tool working on it: close it and try again.

**Settings came back to their defaults** with a message naming them: the settings file could not
be read (damaged, or edited by hand). Everything that could be read is kept, and the old file is
kept as `settings.json.bad`.

## The fine print

Copyright © 2026 Offbeat. Marquee Player is free software: you may use, share and change it
under the GNU General Public License, version 3 or later, and it comes with no warranty.
`LICENSE.txt` beside this file has the licence in full (on Linux, inside the AppImage, in
`usr/share/doc/marquee-player`; on macOS, inside the app, in `Contents/Resources`). Its source code is published with each release: **Settings >
About** gives the address, under the copyright line. It is built on open-source software;
`THIRD-PARTY-NOTICES.txt` lists every component with its licence, and **Settings > About** names
the major ones.
