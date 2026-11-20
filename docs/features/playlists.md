# Playlists

Making, editing and browsing playlists, ordinary and smart, is described with the rest of
the app's lists in [Navigation and keys](navigation.md#navigation-and-keys): [Playlists](navigation.md#playlists), [Selecting several tracks](navigation.md#selecting-several-tracks) and [Smart playlists](navigation.md#smart-playlists). This page is about playlist files.

## Playlist files

Playlists > Import… reads one or more M3U or M3U8 files (UTF-8, else Windows-1252), each into a
playlist named after the file, matching entries to library tracks by path: either slash, any
letter case, `file:///` URLs, and relative entries resolved against the playlist's folder and
then each library folder. Playlist files dropped on the window from Explorer are imported the
same way (audio files dropped with them play, or join the queue, as before).

A file named like a playlist there is already (letter case and runs of spaces aside) is never
imported over it silently. The clash prompt says what each holds ("“Road Trip” has 24 tracks.
The file “road trip.m3u8” brings 31 tracks (3 of its 34 entries are not in the library).") and
offers **Replace** (the playlist gets the file's tracks and keeps its id: home shelves and smart
playlist rules that name it still do), **Keep both** (the file comes in as "Road Trip (2)", the
first number no playlist has; the default, Enter) or **Cancel** (Esc, or a click outside). Over
a smart playlist only Keep both and Cancel. Several files ask once per clash, in turn, the files
without a clash coming in between; while more clashes are to come, "Do the same for the next N
clashes" makes the answer stand for them (Replace for the rest still asks about a smart
playlist). A file none of whose entries are in the library leaves a playlist of its name as it
is, without asking. Then the Playlists page says how each file went (a line each) and lists the
entries that did not match; elsewhere (after a drop) the pill says it too. `playlists/file.rs`,
`playlists/nameclash.rs`.

A playlist's Export…
button, or its menu, saves it as M3U8 (`#EXTM3U` / `#EXTINF`) with absolute paths or paths
relative to the saved file (absolute when on another drive). `library::m3u`.
