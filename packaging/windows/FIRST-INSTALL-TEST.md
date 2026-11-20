# First install test (Windows)

The installer and the portable edition were checked by unpacking and reading them, and the
app's side of every handshake was run here ([docs/releases.md, "Editions"](../../docs/releases.md#editions)), but neither was installed for
real: that registers file types, writes to the Start menu and Apps & features, and would share
the library of the PC it runs on. Run this once, in a throwaway Windows (Windows Sandbox, or a
VM), before handing a release out. About 20 minutes.

Windows Sandbox: "Turn Windows features on or off", Windows Sandbox (needs a restart, Windows
Pro or better). It starts as a clean Windows 11 **without** the Visual C++ runtime, which is
the point. Copy `dist\MarqueePlayer-<v>-setup.exe`, `dist\MarqueePlayer-<v>-portable.zip` (both
made by `cargo xtask release`) and a folder of a few music files into it (copy and paste works). For Windows 10, a Windows 10 VM
(Hyper-V's "Quick Create" offers none any more: an evaluation ISO of Windows 10 Enterprise LTSC).

## Portable, on a Windows without the Visual C++ runtime

1. Before anything: `C:\Windows\System32\msvcp140.dll` is **not** there (Sandbox: it is not).
2. Extract the zip (right-click, Extract All). Run `MarqueePlayer.exe`. SmartScreen may warn:
   More info, Run anyway. It starts: no "VCRUNTIME140.dll was not found".
3. Add the music folder; it scans; a track plays (sound in the Sandbox, if audio is on).
4. Settings > About: "Portable: everything it keeps is in ...\Data". Open folder shows `Data`.
5. Quit from the tray. `%APPDATA%\MarqueePlayer` and `%LOCALAPPDATA%\MarqueePlayer` do not
   exist. `Data` holds settings.json, library.db, the log, cache\.
6. Copy the folder to a read-only place (or set the folder read-only: Properties, Security,
   deny Write for your user) and start it: a message names the folder and says it cannot write
   there; nothing appears in `%APPDATA%`.
7. Optional, with a USB stick: put the folder and the music on it, add the music, quit, plug it
   in where it gets another letter (or change the letter in Disk Management), start it: the
   library is all there, no rescan from scratch, plays and playlists kept.

## Installer, for me only

1. Run `setup.exe` (SmartScreen: More info, Run anyway). The first page asks "Install for me
   only" or "for all users"; choose **for me only**: no UAC prompt.
2. Licence page shows LICENSE.txt (the © sign reads right). Desktop shortcut task unchecked.
3. Finished page: "Start Marquee Player" checked, "Make Marquee Player the default for music
   files" unchecked. Tick both, Finish: the app starts, and Settings opens on Marquee Player's
   page under Default apps (Windows 10: the Default apps page). Choose it for .flac there.
4. Start menu: Marquee Player. Settings > Apps > Installed apps: Marquee Player, publisher
   Offbeat, the version, a size. `%LOCALAPPDATA%\Programs\Marquee Player` has the exe, three
   DLLs, README.md, LICENSE.txt, THIRD-PARTY-NOTICES.txt, unins000.exe.
5. Right-click an .mp3: Open with lists Marquee Player (with its icon). Double-click the .flac
   you made default: it plays in the running app (no second window).
6. In the app: Settings > About says "Installed for you". Settings > Window: "Default app for
   music files" with "Choose default apps…" (opens the same page); "Start with Windows" off.
   Turn it on, sign out and in (Sandbox: skip): the app is in the tray, no window. Task
   Manager > Startup apps lists Marquee Player.
7. Win+R `MarqueePlayer` starts it.
8. Upgrade: with the app running and something playing, run the same (or a newer) setup.exe.
   It does not ask "for me / for all users" again (it keeps for me). On the Preparing page it says Marquee
   Player is running and will be closed; OK: the app quits, setup finishes, the library and the
   queue are there when it starts again.
9. Uninstall from Settings > Apps, with the app running: it is closed first. At the end it asks
   whether to delete the library too: **No**. `%APPDATA%\MarqueePlayer` is still there; the
   Start menu entry, Open with entry, Default apps entry, App Paths and the Run value are gone
   (regedit: HKCU\Software\Classes\Applications\MarqueePlayer.exe,
   HKCU\Software\RegisteredApplications "Marquee Player", HKCU\Software\Offbeat: all gone;
   HKCU\Software\Classes\.mp3\OpenWithProgids has no MarqueePlayer.mp3). The .flac default
   falls back to Windows' own choice.
10. Install again, uninstall again, this time **Yes**: both MarqueePlayer folders are gone.

## Installer, for all users

1. Run setup.exe, choose **for all users**: UAC asks once. It installs into
   `C:\Program Files\Marquee Player`; the keys are under HKLM (`HKLM\Software\Classes\...`,
   `HKLM\Software\RegisteredApplications`).
2. "Make Marquee Player the default…" at the end opens Default apps for the machine-wide entry.
3. Settings > About: "Installed for everyone on this computer".
4. Run setup.exe again with `/CURRENTUSER`: it refuses, saying it is installed for all users.
5. Uninstall (UAC); No to deleting the library.

## Silent

    MarqueePlayer-<v>-setup.exe /VERYSILENT /SUPPRESSMSGBOXES /NORESTART /CURRENTUSER /LOG="%TEMP%\mp-setup.log"
    "%LOCALAPPDATA%\Programs\Marquee Player\unins000.exe" /VERYSILENT /SUPPRESSMSGBOXES /NORESTART

Installs without a window, does not start the app; with the app running, closes it first. The
uninstall leaves `%APPDATA%\MarqueePlayer` in place.

## Windows 10 only

- The window: the caption strip at the top, moving and resizing from the top edge, maximize
  and restore, no stray 1 px line or white strip at the top.
- The text is Segoe UI (not a fallback font): Settings > About looks like Windows 11's, minus
  the variable font's finer weights.
- "Choose default apps…" opens Default apps (not the app's own page): Choose defaults by file
  type works for .flac.
