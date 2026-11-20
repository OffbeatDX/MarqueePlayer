# Metadata and tag editing

Technical details, View metadata, the tag editor and ratings.

## Technical details

Settings > Window > Show technical details (off by default) shows what the music is, quietly,
beside it:

- The player bar, under the artist: the playing track short, `FLAC 24/96`, `MP3 320`,
  `Opus 128`, with the channels when not stereo (`AAC 256 · 5.1`, `FLAC 16/44.1 · mono`).
- Now playing, under artist and album: in full, `FLAC · 24-bit / 96 kHz · 2,915 kbps`,
  `Opus · 128 kbps`, `AAC · 256 kbps · 5.1` (lossless formats by bit depth and sample rate,
  lossy ones by bitrate, the average for a variable one); and under it what reaches the
  device and how loud it is made: `Output: 96 kHz, exclusive, 24-bit` (`, dithered` when a
  16-bit device gets a deeper track), `Output: shared, 44.1 kHz, not resampled`,
  `Output: shared, 48 kHz (Windows resamples from 96 kHz)` (the device's mix format, asked
  of Windows), then `Converted from 44.1 kHz since the crossfade (a pause returns to 44.1 kHz)`
  when a crossfade brought the track in at another rate and the engine converts it (see
  [Crossfade](playback.md#crossfade); "not resampled" is then left out, and the player bar reads `FLAC 16/44.1 → 48`),
  then `ReplayGain −6.2 dB, track` (the tag gain with the preamp and clipping
  protection, without the Opus header gain; `album` when that is the tag used, `No
  ReplayGain tags` when there are none). Settings > Output shows the output line under where
  it plays.
- Album pages: the album's format under its info (`FLAC · 16-bit / 44.1 kHz`; a lossy album's
  average bitrate, or its range when its tracks were encoded very differently). An album of
  one format has no column; a mixed one says `Mixed formats` (`, mostly FLAC · 16-bit /
  44.1 kHz` when one format has most tracks, whose rows then leave theirs out) and its other
  rows show their own.
- Track lists (album, artist top tracks, search, its "See all" page, playlists): a quiet,
  right-aligned column (`FLAC 24/96`, `MP3 320`), only while the page is at least 800 px wide
  (it steps aside on a small window or with sidebars pinned, before it would squeeze titles).
  The queue (sidebar and Up next) shows none: the playing track's details are in the player
  bar beside it.

With the setting off nothing of it is drawn or formatted. The playing track's details are the
library's with what the decoder reports where it differs (the rate it decodes at, channels,
a lossless stream's bit depth); a file opened from outside the library has them read when it
is opened. The engine says what it opened once per track (`StreamInfo`, at `TrackStarted`);
nothing is polled. A track from the server has the library's details (from the server's catalog)
and the decoder's in the same way, with "from the server" after them on now playing, and a Source
row ("From the server") in View metadata's File rows.

Every track's "View metadata" (see [Metadata](#metadata)) lists these details under File, setting or not:
the path, format, bit depth, sample rate, bitrate, channels, duration, size, the ReplayGain tags
present, the date added and the plays (what the earlier Properties panel showed).

The library keeps each track's details (`tracks.codec`, `sample_rate`, `bit_depth`, `bitrate`,
`channels`, schema 7), read by the scan from lofty's properties (`library::audio`, which also
has every wording). What each format gives: FLAC, ALAC, WAV and AIFF their bit depth and sample
rate exactly, and a bitrate of the audio data over the duration; MP3 and MP2 no bit depth (a
lossy stream has none), the Xing/Info average or the frame bitrate, read within 1% of an
encoder setting as that setting (lofty reads a short CBR 320 as 321); Opus 48 kHz always (its
"input rate" is only what the encoder was given) and the stream's average; Vorbis the average
measured from its audio pages (lofty gives the header's nominal bitrate, which can be far off);
AAC the `esds` average. A library from before schema 7 keeps every row as it was; the next full
scan fills in the details of tracks that have none, last and in the background ("Reading file
details" in the sidebar), reading only the files' properties and writing only those columns, a
batch at a time. It is not a re-read: ids, dates, "recently added", art and the missing-tracks
guard see nothing change. A file whose properties cannot be read is marked so and not tried
again until it changes.

## Metadata

"View metadata" ends every track menu (track lists, the queue) and every album menu (album
tiles, the album page's header and its "⋯" button). It opens a wide panel over the window
(at most 960 x 980, centred; the window dims), read-only unless tag editing is on (see [Tag editing](#tag-editing)). It took over the Properties entry:
one entry and one panel for everything about a track, its file's details included.

A track's view, top to bottom:

- **In the library**: what the app shows and uses (title, artist, album, album artist, the
  album's date, genre, disc and track, and what the album is grouped by), each saying why
  where it is not simply the file's tag: "No album artist tag: its tracks' artist", "Read
  from “2019.03.08” as “2019-03-08”", "Credits Harbour Lights, Mara Lind (featured): the
  track is on each one's page", "A compilation: its tracks have no album artist tag and name
  different artists...", "No title tag: from the file's name, “01 - First Light”", "The
  album's, as most of its tracks have it: this file says “Guest Singer”", or that the file
  changed since the last scan (the next scan takes it). A file the app cannot play because of
  its format (5.1 AAC) says so under File.
- **Tags**, then **Identifiers**, **Sort names**, **Loudness** and **Encoding**: every item of
  every tag block in the file, under friendly names in a fixed order (`library::meta::FIELDS`:
  title, artist, artists, album, album artist, track and total, disc and total, dates,
  genre, composer and the other credits, label, catalogue number, release type, status and
  country, media, comment...; ISRC, barcode, ASIN, the MusicBrainz ids, AcoustID; the sort
  names; ReplayGain and R128, with R128 in dB; encoder, encoded by, the Vorbis vendor string).
  A multi-valued item shows each value on its own line. A file with several blocks (ID3v2
  and ID3v1 in an MP3, ID3v2 beside Vorbis comments) says which; a field the blocks disagree
  on gets a line per block, marked ("ID3v1: differs from ID3v2.3"). A number without its
  total and ID3v1's year beside a full date count as agreeing.
- **Pictures**: each embedded picture with its type, format, pixel size, byte size,
  description and a thumbnail.
- **Lyrics**: whether the tags hold any, how long, synced or plain (Copy has the text), and
  whether a `.lrc` beside the file is what the app shows instead.
- **File**: the details listed under [Technical details](#technical-details).
- **Other tags**: every remaining item under its raw key (`TXXX:…`, `----:com.apple.iTunes:…`,
  `COMM:iTunNORM`), last, as it can run to thousands: 300 at a time ("Show 300 more").

An album's view: the album as stored (title, album artist and sort name, date, original and
release dates, genre, label, release type), then what the library keeps (what it is grouped
by: its MusicBrainz release id, or album artist, title and folder; the folder; added;
favourite; plays and last played; tracks and discs; length; size; the format summary), the
cover and where it comes from now (embedded in which file, or which image in the folder,
with its pixel and byte size), **Consistency**, and the **Tracks** table (disc, number, title,
artist, length, format; 300 rows at a time). Consistency lists what the tracks disagree on:
album, album artist, date, genre and MusicBrainz release ("“Patchwork” on 6 tracks,
“Patchwork (Deluxe)” on 1 track"), track numbers missing, used twice or absent, disc numbers
on some tracks only, mixed formats, ReplayGain on some tracks only, and files missing or
unreadable; each track's row says what is off about it, and its number or format is
highlighted when that is what differs. The album's values are the most common among its
tracks; one its files now mostly contradict (changed since the last scan) says so, and a
compilation or an album of several album artists says why. Enter or a click on a row opens that track's view; Back (the
button, Backspace or Alt+←) returns to the album where it was left.

What the database knows shows at once; the files are read off the UI thread (an album's one
at a time, the results arriving in batches) and fill in as they arrive. A file missing or
unreadable is said at the top and where its tags would be. A value longer than 320
characters or 6 lines is cut, with "Show all" (Right and Left expand and fold it; up to
24,000 characters shown; Copy has it all); long unbroken values wrap. Pictures are decoded to
128 px thumbnails off the UI thread and the full images dropped; ones over 48 MB are listed
without one. Results for a view that was closed or left are dropped, and an album's reading
stops when its view closes. Nothing runs while the panel is closed.

Keys: the list is one Tab stop with a cursor over its rows (↑ ↓, Home, End, Page Up/Down);
Enter opens a track, lists more, expands a long value or copies it; Ctrl+C copies the row;
Tab and Shift+Tab go round the list, Back, Copy all, Show in Explorer and Close; Esc closes.
No shortcut acts while it is open, and the focus goes back where it was (the list it was
opened from). A row's copy button shows on hover and on the cursor's row; a double click
copies too; "Copy all" copies the whole view as text (every item, also those not listed
yet). Middle-click scrolling works in the panel's list and never scrolls the page under it.

The viewer writes nothing: `library::meta` opens files with `File::open` (read access,
shared) and only lofty's readers parse them; a unit test fails if that file names a writing
API, and `crates/library/tests/meta.rs` checks the fixtures are byte for byte and timestamp
unchanged after reading (`tests/edit.rs` checks the same of the editor's own reads). With tag
editing off (the default) the panel is exactly this viewer: no row is editable, F2 and Ctrl+S
do nothing, and no writing code path can be reached (`MP_BENCH=editing` checks it).

## Tag editing

Settings > Library > Edit tags (off by default; `settings.json` `tag_editing`) lets View
metadata change files: their tags and their front cover. The hint says so plainly: "Lets the
app change your music files". The app side is `metadata/view.rs` (the panel's rows, editing in
place) and `metadata/tagedit.rs` (the draft, the save thread); the writing is `library::edit`.

**Editing.** A track's view: every field the file has that can be edited, plus the common ones
it lacks (title, artist, album, album artist, track and total, disc and total, date, genre,
composer, comment) as "Not set" rows; "Show every field you can edit" adds the rest (the other
credits, label, catalogue number, release type, status and country, media, script, language,
grouping, subtitle, work, compilation, BPM, key, mood, copyright, the sort names). Enter, F2
(remappable) or a click on a value opens it: one text field per value; Enter keeps it in the
draft, Esc cancels, Tab and Shift+Tab go to the next value or field, Ctrl+Enter adds a value,
Alt+Up/Down moves one, × or an emptied box removes one, Delete on a row removes the field.
Values are checked as typed, forgivingly: a number is digits ("3/12" typed as a track number is
number and total), a date any form the scanner reads ("2019.03.08", "2019/3/8", "20190308"
become ISO), the compilation flag yes or no. Identifiers (MusicBrainz ids, ISRC, barcode, ASIN,
AcoustID) can only be removed (or kept: the row's button and Delete toggle); loudness
(ReplayGain, R128), encoder details, lyrics, rating and items under raw keys are shown, never
changed (the lyrics can be written whole from the Lyrics tab, "Save into the file's tags", through
this same save: see [Lyrics](lyrics.md#lyrics)). An album's view edits its title, album artist, date, genre, label, total discs and
compilation flag once for every track ("Editing a field here sets it on all 12 tracks"), and its
track table in place: F2 on a row edits its number, title and artist (Tab between them, Up and
Down to the row above or below; Enter on a row still opens the track). Each inconsistency the
album's checks find offers its obvious fix as a draft to review, never applied by itself: the
value most tracks have on the tracks that differ (album, album artist, date, genre), the tracks
numbered in file order per disc, disc 1 filled in where it is missing. The cover: replace, add
or remove the front cover (Pictures; an album's Cover, on all its tracks) from an image picked
in a file dialog; a JPEG or PNG up to 2000 px and 3 MB is embedded as it is, anything larger (or
WebP) is saved again as a JPEG (quality 90) at most 2000 px, transparency laid on white; the
draft shows its thumbnail, size and on how many files it goes.

**The draft.** Nothing is written until Save. Changed values are marked (a bar at the row's
left, the label in the accent, "Was “…”" under the value, "Before: …" under a table row), the
bar under the header says "3 unsaved changes in 2 files · not written to the files yet" with
Review and save (Ctrl+S) and Discard, and "In the library" shows what the library will make of
the draft ("Once saved, the library shows “…”", the scanner's own rules: `metadata::view::with_draft`).
A track and its album share one draft. Closing the panel (Esc, Close, a click outside) with a
draft asks first. Review lists every file and every change, old to new; Save writes them on a
thread of their own, one file after another, with progress and Cancel (between files, never in
the middle of one); the result says what was written, and if a file failed, which one and why,
which were not tried, and offers to undo what was written. What was not written stays a draft.

**A file's save** (`library::edit::save_file`), each step refusing the file (it is left as it
was) when it fails:

1. The file is there, a file, not read-only, and as it was when the draft was made: its size,
   its modification time and a hash of its tags (an undo: its whole content's hash, as the save
   left it). Otherwise "the file changed on disk after you started editing it".
2. Its tags are read as stored (lofty's implicit conversions off: ID3v2.3 keeps TYER and TDAT,
   MP4 its `gnre`) and the field edits planned as changes to items of its blocks
   (`edit/fields.rs`): the block the library reads (lofty's primary tag, else the first the file
   has) and every other block that already holds the field, so they agree. A file whose only
   tag holds few fields (ID3v1, RIFF INFO, AIFF text chunks) gets an ID3v2.3 tag with what that
   tag held copied in, so the library reads what it read before and the edit; the old tag is
   kept, in step. Picard's ARTISTS follows ARTIST when the file has it (the scanner credits
   ARTISTS first). Multi-valued fields: Vorbis comments and APE repeat or NUL-separate, ID3v2.4
   NUL-separates, MP4 has a data atom per value; ID3v2.3 and RIFF INFO hold one value, so
   several are joined with "; " (which the scanner splits again; the result says so).
3. It is copied to `<name>.mpedit-<token>.tmp` beside it (same volume; after a record of the
   names in the data folder's `edits\inflight`), and the copy changed with lofty, only the
   blocks that change, the version the reader uses (`edit/write.rs`). ID3v2.3 stays 2.3 and
   keeps its padding size; FLAC gets no padding block it did not have.
4. The copy is checked (`edit/check.rs`): its tags read back (`meta::read`, raw) as the
   original's with exactly the planned changes, values and binary data; its pictures the
   original's but the changed front cover; the file's own tag items read apart from lofty
   (`edit/raw.rs`: ID3v2 frames, Vorbis comments, MP4 `ilst` atoms, APE items, RIFF and AIFF
   chunks, ID3v1) the same outside what changed, byte for byte where lofty does not re-encode
   (unknown ID3v2 frames, binary data), so nothing lofty does not show can be lost; the ID3v2
   version and padding and the FLAC padding blocks unchanged; the audio unchanged three ways:
   the bytes outside the tags (MPEG frames between the tags, FLAC frames and STREAMINFO and
   every other block, the Ogg packets with their granules, MP4 `mdat` and the sample tables
   with chunk offsets taken relative to it, WAV and AIFF chunks), every packet Symphonia (the
   player's demuxer) reads with its time stamps and trimming, and lofty's duration, sample
   rate, channels and bit depth.
5. What undoes it goes into the journal, flushed; then the copy, flushed to disk, replaces the
   file with `ReplaceFileW` (the original's creation time, attributes, security and alternate
   streams kept; the original moved to `<name>.mpedit-<token>.bak` by the same call and deleted
   after; put back if the call fails half way). The new file's modification time is now, and at
   least a second past the old one's, as the scanner compares whole seconds.

At every moment the file's name holds the old file or the new one, whole. After a crash or a
power cut, the next start (the first instance only) reads the records left in `edits\inflight`:
a name left empty gets its backup back, and the temporary and backup files go; nothing else in
the music folders is looked at. A record whose folder is not there at that start (a USB drive not
plugged in, a share not reachable) is kept for a later start rather than dropped.

Other volumes: where `ReplaceFileW` reports it cannot do the swap (some network shares and NAS,
FAT drivers without the information classes it uses), the same swap is made by two renames (the
original to its backup name, the checked copy to the original's), covered by the same record; a
crash between them is mended at the next start as above. FAT32 and exFAT have no journal: a
power cut during a rename itself can leave the drive needing its own repair (`chkdsk`), whatever
the program. Not run on a real FAT32, exFAT or SMB volume (none was at hand, and making one
needs administrator rights): the fallback is unit-tested at the file-system seam
(`edit::atomic::tests::a_swap_by_renames_is_whole_or_untouched_and_recovery_waits_for_an_absent_drive`). Read-only files, full disks, missing drives and network paths,
files held by another program: refused per file, with the reason. A save of several files stops
at the first that fails (Cancel stops after the file being written).

**A playing file.** The engine opens files with `File::open`, which shares delete: the
replacement goes through while it plays; the engine reads on from the old data (Windows keeps it
until the handle closes), as does a pre-opened gapless next track, and the next play opens the
new file. The audio is the same by the check, so nothing is heard. `tests/edit.rs` decodes a
file through the player's own `Source` while it is saved (a grown tag and a new cover) and
compares every sample; `MP_BENCH=editing` saves a track while it plays. A program holding a file
without that sharing makes its save fail ("another program is using the file"), untouched.

**The library** takes the new tags the way it takes any retag: the folders of the files written
are scanned at once (`scanning::rescan_changed`, what the folder watcher asks for too; its own
ask a few seconds later finds nothing to read), so ids, plays, favourites, ratings, playlist
entries and dates stay with the tracks, and an album whose identity changed (a new title, album
artist or MusicBrainz release) is regrouped by the scanner's rules, its favourite and "added"
date going with it (`scan.rs` test `the_editors_own_writes_come_in_like_any_retag`). The panel
reads the files again after a save.

**Undo.** Edit history, at the bottom of a track's or album's view, lists the saves that wrote
its files (newest first); Undo… reviews and writes the previous values back (and the previous
front covers), through the same checked replacement, and refuses a file not as that save left it
(another program changed it, or a later save: undo that first). An undo is a save itself, so it
can be undone. The journal (`edits\saves\<id>.jsonl`, a line per file appended before the file
is replaced; covers once each in `edits\pictures`) keeps the last 100 saves within 256 MB, oldest
dropped first, never the newest; Settings > Library > Undo history shows its size and clears it.

**Formats** (each a round-trip test in `tests/edit.rs`: one field edited, every other item, every
picture and the decoded audio, by ffmpeg, compared): FLAC; MP3 with ID3v2.4, with ID3v2.3 and
ID3v1, with ID3v2.3, APEv2 and ID3v1, with unusual frames (PRIV, GEOB, PCNT, POPM, WXXX); MP2;
Ogg Vorbis (a picture in METADATA_BLOCK_PICTURE, a lower-case key); Opus; AAC and ALAC in MP4
(`.m4a`, `.m4b`); WAV with ID3v2 and RIFF INFO, and with RIFF INFO only; AIFF with ID3v2 and
text chunks, and with text chunks only; files with no tags at all. Not edited, with the reason
shown: raw AAC (`.aac`: no tag format of its own, and lofty estimates its length from the file's
size, so the check that only the tags changed cannot pass), ID3v2.2 tags (old three-letter
frames the editor would have to convert), Speex (not played), a file over 2 GB (the check
reads a file whole, the original and the copy one after the other), and a file with more tag items
than the editor reads (20,000). FLAC's ID3v2 tag (rare; lofty reads it only) is left as it is.
ID3v1: when a field it holds (title, artist, album, year, comment, track, genre) changes, it
follows, within its limits (30 characters, Western European letters, its list of genres; the
result says when a value was cut or cleared). Vorbis comment names are compared without case
(lofty writes them upper-case). An emptied block goes (lofty writes no empty tag), and an
encoder's empty FLAC comment block with it.

**Files.** Files outside the library (opened from Explorer) can be edited too (no rescan then);
missing files cannot (nor files on the missing-tracks list: they are missing). Tests and the
bench may write only fixtures in a folder marked `.mp-edit-fixtures` inside the system's
temporary folder: `library::edit::atomic::set_guard` refuses every other path, and every bench
build sets it.

## Ratings

Tracks can be rated 1 to 5 stars: "Rate…" in a track's menu, the star beside the heart in
the player bar, or `1`…`5` for the playing track (`0` clears; these keys can be remapped). Track lists
show the stars of rated tracks. Ratings are kept in `track_ratings` (schema 6), apart from
the scanned rows, and go with a track when it leaves the library. Those of the tracks a Navidrome
server has are kept in step with it, both ways (see "What goes back to the server", under
[Navidrome](navidrome.md#navidrome)).
