# Library and scanning

How folders become the library: scanning, moved files, tags, covers, dates, the home page and the formats played.

## Scanning

A scan (`library::scan`, driven by `scanning.rs`) walks the library folders, reads the tags of
the files that are new or changed (by modification time and size), and writes the library a
batch of whole album folders at a time, each batch in a transaction of its own
(`scan/batch.rs`): the first albums are in the library, on Home, in Albums and in search within
seconds of adding a folder, and can be played while the rest is read; their covers follow a
moment later. A first scan reads folders as the walk finds them (`scan/walk.rs`); a later one
walks everything first (it has to know what is gone before it can tell a move from a new file).

**Stopping.** Quitting, a crash, the PC asleep or the drive gone mid-scan loses at most what
was not yet written (the files being read, under a second's worth); Settings > Library has
**Stop**, which writes what was read and ends. The next scan, at the next start or by
**Resume**, finds the written files unchanged and reads only the rest, and says it picks the
last one up (`scan_state`, schema 10: a row from the first batch to the end of a scan). A stopped
scan removes nothing and does not run the missing-tracks guard; the next whole scan does both. Stop once
every file is read and written, while covers are still looked for, leaves those covers to the
next scan and says so ("Stopped", with Resume, `Report::covers_cut`), not that the scan finished.

**What a batch boundary must not change** (`batch.rs` has the reasoning, the tests prove it):
- an album is decided with all the tracks of the folder it is grouped in, and a unit of work is
  every file to read of one group folder (with its disc subfolders and sibling disc folders),
  so a batch never holds part of an album; one MusicBrainz release in several folders keys by
  its release id whatever the batches, and is worked out again from all its tracks each time;
- when files vanished, a new file that could be one of them (the same title, numbers and
  duration) and the folders vanished tracks were in are held back and written at the end, with
  the removals and the guard, in one transaction, as every scan used to be: a moved library
  keeps every play, favourite, rating and playlist entry. A first scan has nothing to hold
  back; a folder watcher's scan with something gone under its folders holds back everything
  (it may hand over to a full scan). Held files are read again if the scan is stopped before
  its end;
- a first import's albums are dated by their files (see [Recently added](#recently-added)), the same whatever
  the batches; later, an album is stamped when its batch is written.

`scan::tests` (`a_first_scan_stopped_after_every_batch_ends_as_one_that_was_not`,
`a_rescan_with_moves_retags_and_removals_stopped_after_every_batch_ends_as_one`, the stopped
import, the stopped prune, the slow disk) and `tests/progressive.rs` (the messy fixture
library, a first scan and a reorganised rescan) stop scans after every batch and check that the
end is the library one uninterrupted scan makes, row for row in everything a user sees and all
of their data (`tests/support/scan_dump.rs`), and that no file is read twice.

**Sharing the database.** A batch holds the database for tens of milliseconds at most (512
files; measured on the 29,000-track fixture: median 28 ms, at most about 50); the names of the
artists and genres the batches touched are worked out again from all their credits every few
seconds, a few at a time, each in a short transaction of its own (until then a new artist shows
the spelling it first came with), and the WAL is checkpointed by the scan itself, outside any
transaction (`db::let_waiting_in`, the busy handler: a write of the app's own, such as a play
counted on the UI thread, is let in before the scan's next batch).

**What it says.** The foot of the sidebar, Settings > Library and the top right of the home page
show what the scan does ("Adding your music", "Picking up where the last scan stopped",
"Scanning your folders"), how far ("4,120 of 29,047 files read · 312 albums in the library"), a
progress line once the total is known, about how long is left once a rate is (over the last half
minute of reading), the folder it waits on when one is slow, and "Waiting for the disk: nothing
read for 12 s (in …). Is the drive still connected?" when it has stalled. An empty home page says
the same in its own large words, under the title.

**The top right of Home** (`activity.rs`, the `Activity` global) shows every library job: the first
import, a rescan, the folder watcher's scan, a folder's removal, and every sync with the server
(see [Navidrome](navidrome.md#navidrome)), a folder or a cloud saying which. Its line is empty until the total is known,
then fills. It used to show only during a first import: it was tied to `Library.adding` (a first
import or a resumed one), and a quick rescan was over before its first report of progress anyway,
so a rescan never showed. Now a job shown stays at least 700 ms, and when it ends its line runs to
full and stays there 450 ms before it goes. A job that comes right after (a rescan asked for during
a scan, or a sync after a scan: the two take turns writing the library, `server::sync::WRITER`)
takes its place then, with no gap; a scan that starts during a sync is shown at once (the sync
waits for it) and the sync comes back after it, even when it ended meanwhile. Nothing runs while
nothing happens: it is told when a job starts, says how far it is or ends, and its one timer is a
single shot for the end of the hold.

**Open pages.** What a scan writes reaches the page that is open at most every 2.5 s, in place
(`views::library_changed_live`): Home's shelves keep the albums they show and take new ones after
them; the Albums page keeps its albums where they are and adds the new ones after them, taking
its order again when it is next opened (or at the scan's end, if it is at its top); covers come
in as their albums' rows change. The browse indexes and smart playlists are worked out again at
most every 15 s while a scan runs; pages not open are read again when shown; the queue and View
metadata at the end, as before. Nothing runs between batches.

**On a slow disk.** `MP_SLOW_DISK_MS=<ms>` simulates one: every folder listed and file opened
by the scanner takes that much longer, one at a time (a single spinning disk, a share whose round
trips do not overlap). Measured with 8 ms on the messy library (2,754 tracks, 271 albums), the
app under the bench harness (`slowscan`): the first albums on Home 0.4 s after the start (32 s
before this change, when everything came at the end of a 32 s scan), the whole scan as long as
before, the app's own writes never waiting more than a few milliseconds, and a quit 40% through
keeping 1,126 tracks, the restart reading only the other 1,628. What the simulation cannot tell:
how a real disk orders concurrent reads (sixteen readers on one spindle may be slower than
the simulated single lane), a share's own caching and timeouts, and a drive that disappears
(reads that fail or hang rather than wait).

## Moves, renames and missing folders

Plays, favourites and playlist entries belong to a track's row, and a track is its file
path. When a scan finds files gone and new files that are the same files moved or
renamed, it re-points the existing rows to the new paths instead of removing them and
adding new ones, so the history follows the music. A new file counts as a moved one only
when the duration, title and disc and track numbers agree, and also the size (the file
itself is unchanged), or the file name (retagged and moved), or the artist and album
(renamed and retagged); and only when it is the one candidate both ways. Two copies that
look alike match nothing: the old row goes and the new files come in fresh, as before. A
folder renamed only in letter case is the same folder (Windows paths ignore case).
Favourite albums follow an album that comes back under a new identity, as its date does.

A library folder that still exists but where nearly everything is missing (at least 25
tracks, and fewer than one in ten still found: a network share mounted empty, a drive
letter pointing at an empty folder, a library mid-reorganisation) is not pruned. Its
tracks stay, Settings says so beside the scan status, and "Remove missing tracks" removes
them when that was intended. Deleting some albums, even most of a library, prunes as
usual. A folder the scan cannot read (a share's error, no permission, an online-only
folder whose provider is not running, a link to a drive that is not there) is not taken
for empty: its tracks are held the same way, Settings names the folder and the error (it
is in the unreadable list too), and they stay until it reads again; "Remove missing
tracks" does not remove them, and is not offered when nothing else is held. Before any track is removed its file is looked for once
more, and only a plain "not there" removes it. Removing a folder from the library (Settings) runs in the background; during a
scan it waits for the scan to finish.

A change the folder watcher reports is scanned only in the folders where it happened. A
new file there may still be a track moved from elsewhere: tracks of the same duration,
numbers and title whose file is gone are matched by the rules above, wherever they were.
Such a scan never removes anything: if something under its folders is gone and is not a
move, a full scan runs instead, so the removal and the guard above always see the whole
library.

A library folder that is not there (a drive unplugged, a share down, at start or while
watched) is not watched, and the scan holds its tracks as above. While any folder is
unwatched the watcher looks for it every 30 s (nothing wakes while all are watched); once it
is back it is watched again and a scan runs, as what changed meanwhile was not seen. A
folder that is there but cannot be watched is tried less and less often, up to every 30 min.

## Artists, albums and imperfect tags

How the scanner (`library::scan`, `tags`, `names`, `group`, `relations`) turns tags into the
library, and what it does when tags are missing or disagree. The scanner never writes to the
files (only tag editing does, when the user saves); every fallback below is deterministic (the same files give the same library, whatever
order they are read in) and View metadata says, under "In the library", where each value
came from.

**Several artists and genres.** A track belongs to every artist it credits and an album to
every genre its tracks carry (schema 8: `artists`, `track_artists`, `album_artists`, `genres`,
`track_genres`, `album_genres`; `relations.rs`). Every value of a multi-valued tag is read:
Vorbis comments and APE repeat the key, ID3v2.4 separates values with NUL, MP4 holds several
data items (lofty hands each value over). Picard's ARTISTS (each artist of the credit) wins
over taking ARTIST apart, and MusicBrainz artist ids are kept where there are as many as names.
A single string that joins several is split on ";" and on " / " (a slash with spaces), never on
",", "&" or a bare "/": "Earth, Wind & Fire" and "AC/DC" stay one artist, and ID3v2.3's
conventional "/" is split only spaced. "A feat. B", "A ft. B", "A featuring B" and "A (feat.
B)" credit A, and B as featured; in a title, only a bracketed credit counts ("Song (feat. B)").
Genres split the same way ("Hip-Hop/Rap" and "Drum & Bass" stay whole). What is shown is the
credit as tagged: ARTIST as written ("A feat. B"), or several values joined with "; "; each
credited artist's name in it is a link to their page.

**One name, one artist.** Names that differ only in letter case, surrounding or repeated
spaces or Unicode normalisation form (NFC) are one artist, genre, label or album title, shown
in the spelling most of its credits use (ties: code point order). Accents are part of a name
("Sigur Ros" and "Sigur Rós" stay two; search finds both). An artist with a MusicBrainz id is
one artist by that id; credits of the same name without an id join it when exactly one artist
with an id has that name (two same-named artists with ids keep the plain-name credits apart,
as which one they mean cannot be known). An artist's sort name is the most common ARTISTSORT of
tracks that credit only them or ALBUMARTISTSORT of albums that are only theirs, and it sorts
all their albums on the Albums page and in the Artists index, also those tagged without one.

**Which album a track is on** (`group.rs`). By its MusicBrainz release id when tagged; else by
album artist, album title and folder. The folder is the track's, or the album's for a disc
subfolder ("CD1", "Disc 2", "disc_02", "Disc 1 - Live") or a sibling folder named for its disc
("Live CD1" and "Live CD2" are one album). The album artist is decided with all the tracks of
the folder that share the title:
- album artist tags that agree: theirs, also for the tracks that lack the tag;
- album artist tags that disagree: one album each, as tagged (unless the compilation flag is
  set: then one compilation);
- no album artist tag: the first credited artist, when all tracks have the same ("A" and "A
  feat. B" are one album by A); otherwise a compilation, "Various Artists", when a track is
  flagged as one (TCMP, cpil, COMPILATION) or when every track has an album tag and no two
  share a disc and track number (two different albums with one title in one folder repeat
  their numbers, and stay apart, one per artist); otherwise one album per artist, as before.

A folder of loose singles without album tags stays one album per artist; "Various Artists" or
"VA" as album artist is a compilation.

**Album values.** An album's title, album artist, sort name, dates, genre, label and release
type are worked out from all its tracks, so they no longer depend on which track a scan read
last: the most common value among the tracks that have one (spelling by `norm`), ties going to
the album's first track (by disc, number, path). For the date, the most common year wins and,
of that year, the fullest date given ("2004" on six tracks and "2004-03-15" on four give
"2004-03-15"). Its genres are every genre of its tracks, the most carried first; its folder and
art are its first track's.

**Missing values.** No title tag: the file's name, without a leading number. No track number
tag: the file's name ("01 - Title", "01. Title", "01_Title", "01) Title", "01 Title" with a
leading zero, "1-03 Title" for disc 1, track 3); "4 Minutes" or "99 Balloons" are titles. No
artist: the album artist; neither: "Unknown Artist". No album tag: the folder's name. A track
without a disc number is on disc 1, so an album with disc numbers on some tracks shows "DISC 1"
once (or no disc header at all when there is one disc). Numbers taken from a file's name are
not used to recognise a moved file (a rename changes them).

**Not fixed, on purpose** (it would be a guess the user could not see or undo):
- two spellings of an album title that differ in more than case and spacing ("Patchwork" and
  "Patchwork (Deluxe)") stay two albums; View metadata's Consistency names them;
- tracks of one album whose album artist tags disagree stay one album per album artist (only
  the compilation flag joins them);
- vinyl positions as track numbers ("A1", "B2") are not numbers: such tracks are listed in file
  name order, unnumbered, and View metadata says so;
- a group credited by its name with its members listed (ARTIST "The Carters", ARTISTS Beyoncé
  and JAY-Z): the credit is shown as the members' names, linked;
- artist names in different scripts or romanisations are different names;
- an album whose tracks have different embedded art takes its first track's.

**Libraries from before schema 8.** The migration (one transaction, a copy kept as
`library.v7.bak.db`) adds the tables and fills them from what the database holds, as the
artist pages always read it (split on ";" and " / ", featured credits), so every page shows at
least what it showed. Nothing is re-read then. The next full scan reads every file's tags again
in the background ("Updating the library" in the sidebar), a batch of whole folders at a time,
each in its own transaction (`scan::fill_tags`), and regroups albums by the rules above: albums
that merge (a compilation, an album split by a featured credit) keep the oldest "added" date
and every favourite; plays, ratings, playlist entries and the queue belong to tracks, whose
rows (ids) never change; art is cached again only for albums that changed which tracks they
hold. Measured on 2,754 fixture tracks: 0.19 s for the re-read, 358 albums becoming 271, no art
re-made, every play, rating, playlist entry and favourite kept; the scan test
`regrouping_an_old_library_keeps_every_piece_of_user_data` checks the same on a small library.

**Online services.** Last.fm's `artist` and LRCLIB's `artist_name` are documented only as "the
artist name"; Last.fm asks for metadata as tagged, not guessed. Both get the credit as tagged
("A feat. B", how both list such tracks), except a list joined with ";" or " / ": a joined list
is no name either service knows, so its first artist (`names::service_artist`). Last.fm's album
artist the same way ("Various Artists" for a compilation).

## Art cache

Each cover is cached as a 360 px thumbnail (JPEG quality 85), a 1200 px copy for the
album page and now playing (quality 88), a small blurred backdrop and its accent colour
(`library::art`). The cache's format is in `art\version.txt`; when it changes (sizes or
qualities), the next scan re-makes every cached cover from its source once, in place,
the old images showing until the new ones are written. An album without art is looked at
again only when its folder changes (a cover added), not on every scan. Albums only a Navidrome
server has get their covers from it into the same cache (see [Navidrome](navidrome.md#navidrome)).

Covers no album uses any more are removed from the cache (`ArtCache::sweep`): after a scan,
folder removal or server sync that removed albums or looked at their art again, and otherwise
once per start, after the first library job; on that job's thread, while no other scan or sync
writes. Only the cache's own files in `art\` go (`<stem>_t.jpg`, `_f.jpg`, `_b.jpg`, `_a.txt`
and the temporary names of a write cut short; never `version.txt`, a folder or anything else),
only for a cover no album refers to, and only when none of its files changed in the last hour:
a cover being stored, or found cached again (which touches it), is never taken for unused.
Signing out of a server leaves its covers until the next start's sweep.

## Recently added

An album's "added" date (`albums.added_at`, Unix milliseconds) is stamped when the scanner
first sees it and kept through rescans and retags. If an album comes back under a new
identity (a retag adds its MusicBrainz id, or an untagged album's folder is renamed or
moved), the new row takes over the old one's date, so it does not jump to the front. Albums
that merge (see [Artists, albums and imperfect tags](#artists-albums-and-imperfect-tags)) keep the oldest date of their parts.

A first import is the exception (an empty library, or the rest of one that was stopped; see
[Scanning](#scanning)): every album of it would be "first seen" within the same minutes, in the order the
scan happened to read the folders, which says nothing. Its albums are dated by their files
instead: the earliest moment any of an album's files is known to have been on the disk (its
creation, or its last change when that is earlier, as a copied file is created anew but keeps
the time it was changed), never later than the scan, `albums.added_source` `files`
(`first_seen` is the real first sight). So Recently added after a first import shows what was
added to the collection last, like Navidrome's file dates, and the dates are the same whether
the import was interrupted or not (the earliest over all of an album's tracks, whichever batch
each came in). From then on albums are stamped when first seen, as above.

Earlier versions could set these dates once from a Navidrome database. Albums dated that
way keep the imported date: `albums.added_source` says `navidrome` for them, and the
scanner's own date is still in `albums.first_seen`.

## Home shelves

The built-in shelves use the sorts of Navidrome's album lists (0.64,
`persistence/album_repository.go`, `sql_base_repository.go`), so a library shared with a
Navidrome server orders its shelves the same way. All are album shelves; compilations, singles
and EPs are treated alike. A shelf shows its first `limit` albums (20 by default). A shelf's
title, or "See all" beside it, opens the whole list on its own page (the album grid, same
order; Back and Forward, scroll and the pill work as on any page). A random shelf's page starts
with the shelf's albums and can be reshuffled; a playlist shelf opens the playlist.

| Shelf | Navidrome's album list | Here |
| --- | --- | --- |
| Recently added | `recently_added`: `album.created_at` desc (oldest file birth time), then `album.id` | `added_at` desc (first seen, or the date an earlier version imported from Navidrome), then id |
| Recently released | `max_year`: original date if tagged, else the year; then release date, then name; all desc; undated last | the same, but with the full date when there is no original date (Navidrome compares only the year then); dates like `2026.07.12` read as ISO |
| Recently played | `play_date` desc: the album's latest track play; never-played albums follow | latest play desc; only played albums |
| Most played | `play_count` desc: plays of all its tracks, all time; never-played albums follow | the same, only played albums, ties by latest play |
| Random albums | `random`: a new shuffle on each request | seeded shuffle kept until Reshuffle, so Back shows the same albums; the hero takes only albums with art |
| Favourite albums | `starred`: `starred_at` desc | favourited time desc |

A play counts after 75% of the track or four minutes of listening, whichever comes first.

The default page names every shelf for what it holds, and no two neighbours are alike:
Recently added as the big card (the newest album; the next eleven in the card's strip), then
Recently played, Random albums, Most played, Recently released and Favourite albums. A shelf
with nothing in it is not shown, so a new library starts with what was added, a random row and
the releases. Earlier versions opened with two random rows (a "Featured" card whose row of the
rest was titled "More", then "Explore"): a settings file that still has exactly those defaults
gets the new ones; shelves changed in any way are kept as they are. Settings > Home > Edit
shelves > Reset to defaults applies them to any setup.

A hero shelf (any query, chosen in the shelf editor) is one card: its first album big over its
blurred art, labelled with the shelf's title (which opens the whole shelf), and a strip of the
shelf's other albums (as many as its limit and the width allow) inside the card. Choosing one
(a click, or the arrow keys on the strip, which is one Tab stop) features it in the card; the
strip rings the one shown. Nothing under the card needs a title of its own.

## Formats

Opus, Ogg Vorbis, FLAC, MP3, MP2, WAV, AIFF (`.aif`, `.aiff`, `.aifc`), AAC (`.m4a`, `.mp4`,
`.m4b` audiobooks, raw ADTS `.aac`) and ALAC. AAC in MP4 is gapless: the encoder delay and
padding come from the track's edit list or iTunes' `iTunSMPB` (`player-core/src/mp4gapless.rs`;
Symphonia does not apply them). Raw ADTS and MP2 carry no such information, so their track
boundaries are not sample-exact. WavPack and Monkey's Audio are not supported: the only
WavPack decoder for Symphonia is a third-party crate (GPL-3.0, which the app's own licence
allows), and there is no Symphonia decoder for Monkey's Audio.

Symphonia's AAC decoder, the only one the app has, plays AAC-LC in mono and stereo only: a
5.1 (or other multichannel) AAC file, or an HE-AAC one, cannot be played. The "Can't play"
note says so ("AAC with 6 channels (5.1)"), as does View metadata, so it is clear the format
is the reason, not a damaged file. No other decoder is among the dependencies; FLAC, Opus or
WAV multichannel files play.

Files with more than two channels are mixed down to stereo by their channel layout
(`player-core/src/downmix.rs`): the centre into both sides at -3 dB, surrounds and backs to
their side at -3 dB, the LFE left out, scaled so that nothing can clip.
