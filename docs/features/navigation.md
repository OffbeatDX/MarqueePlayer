# Navigation and keys

Moving around the app: the sidebars, browsing, search, playlists and their selection, smart playlists, the keys, middle-click scrolling, the tray icon and files opened from outside.

The queue lives in a sidebar on the right, the left one's mirror: it slides in at the
right window edge or on the player bar's queue button, and `Ctrl+Q` (unless remapped) pins or unpins it. Drag
its rows to reorder, × to remove, double-click to play. Drag an album cover or a track
row from the page onto it to insert it where the line shows; dragging toward the right
edge while it is hidden opens it, and a drop on the edge itself appends. Its rows can be
chosen several at a time, as the page's can (see [Selecting several tracks](#selecting-several-tracks)). A pinned
sidebar (left or right) reserves its room; an unpinned one slides over the content.

Settings > Window > Auto-hide player bar slides the bar away while music plays and brings it
back when the pointer reaches the bottom of the window (the strip where the bar sits). It
stays while the pointer is anywhere on it, its buttons and sliders included, while a slider
of it is being dragged, while one of its controls has the keyboard focus (Tab brings it
back), and while a menu is open. Once nothing holds it, it
lingers for `[motion] bar_linger` in `theme.toml` (600 ms) before going, also when the
pointer leaves the window. Paused, stopped or on Now playing it stays. While it is away
the sidebars and the queue reach the bottom of the window; they give the bar its room back
as it slides in. The bar's empty parts take clicks, so a click there never reaches the
page under it; the wheel still scrolls the page.

There is no Windows title bar: the page runs to the top edge. Drag along the top to move
the window (to a screen edge to snap), double-click it to maximize or restore, right-click
it for the window menu. Minimize, maximize and close show when the pointer nears the top
right corner; hovering maximize offers the Windows 11 snap layouts. Close (and Alt+F4)
hides to the tray; Settings > Window > Close button can make it quit instead.

The pill at the top left opens the sidebar (☰), goes back and forward, and names the
current page (a long title or note is cut short with "…"). The sidebar also slides in at the
left window edge; `Ctrl+B` (unless remapped) pins or unpins it.
History is per place: Back from an artist returns to the album you came from.
A page change fades the new page up and slides it in from the side it comes from
(right going forward, left going back) over `[motion] page` in `theme.toml` (180 ms;
0 turns it off).

## Browsing

The sidebar has Albums, Artists and Genres. Albums sorts by artist, title (both in natural order:
"Vol. 2" before "Vol. 10", case ignored; so does the playlist list), release date,
date added or most played, either way round (the sort button, then the arrow beside it;
the choice is kept in `settings.json`), and the filter box narrows the grid as you type
(every word somewhere in the title or the artist; case, accents, kana and width ignored;
Enter goes to the covers, Esc clears it). Artists shows every album artist (each artist of a
shared album on their own, compilations under Various Artists) with the cover of their latest
album; an artist's page has their albums, the albums they appear on (a track credits them, as
an artist or featured) and their most played tracks. Genres, and Labels beside it, open that
genre's or label's albums (an album is under every genre its tracks carry). Artists, genres and labels sort by name in
natural order: case ignored, numbers by value ("Genre 2" before "Genre 10"). Search folds accents both ways ("beyonce"
finds "Beyoncé"), finds featured and track-only artists too, and each section with more
than it shows has "See all", which opens it in full as its own page in the history. Search runs
on a thread of its own (`mp-search`), "See all" too: its page opens when the section is found (if
the user is still where they clicked it), so a slow operator search never holds up the window.

## Search operators

The search box also takes operators (`library::smart::search`). A query with at least one is
read into smart playlist rules and evaluated by the same SQL a smart playlist is (see [Smart
playlists](#smart-playlists)); a query without any is plain search, exactly as before, on its own path.

| Typed | Means |
| --- | --- |
| `artist:"nina simone"` | a text field contains it (folded as search folds: case, accents, kana, width): `artist` (every credited artist, featured too), `albumartist` (`aa`), `album`, `title`, `genre`, `label`, `format` (`codec`), `path` |
| `label:=ECM` | exactly that (folded) |
| `live` | a word: in the title, an artist, the album or the album artist; each word on its own, a `"quoted phrase"` as one |
| `year:1994`, `year:1990..1999`, `year:1990s`, `year:>=2010`, `year:..1980` | numbers: `year`, `plays`, `length`, `bitrate` (kbps), `samplerate` (kHz, or Hz from 1000), `bitdepth`, `disc`, `track`; with `>`, `>=`, `<`, `<=`, `=`, `a..b` (either end open) or `n+` |
| `length:<3:00`, `length:90s`, `length:5m..8m` | lengths as `3:30`, `1:02:03`, `90s`, `5m`, `1h`, or minutes alone (`length:>10`), as the editor reads them |
| `added:<30d`, `played:>1y`, `played:<2w` | in the last 30 days; longer ago than a year (never played included); `d`, `w`, `m` (months), `y` |
| `added:2024`, `added:2024-03`, `added:>2023`, `added:2023..2024-06`, `played:never` | a year, month or day, after it, before it, between (local calendar, as in the editor) |
| `is:favourite` (`favorite`, `fav`), `is:favalbum`, `is:local`, `is:remote`, `is:lossless`, `is:lossy`, `is:played`, `is:unplayed` | flags; `is:remote` is what plays from the Navidrome server (a track only it has), `is:local` what has a file here |
| `playlist:"Road trip"` (`in:`) | in that playlist (ordinary or smart), by name |
| `-genre:live`, `-is:remote`, `-live` | not; a negated word is an operator by itself (`jazz -live`, or `-live` alone: every track without the word) |
| `genre:jazz OR genre:soul` | either (`OR` in capitals, or `|`); it binds its two neighbours, everything else must all match |

Field names, flags and units ignore case (`Artist:`, `IS:Fav`); `artist: nina` (a space after
the colon) works too. There are no parentheses: `OR` joins the terms on either side of it into
one. Terms are ANDed. Negation uses the rules' own opposites, so a query saved as a smart
playlist stays editable in the editor: "contains" and "does not contain", "is at least 4" and
"is at most 3", "in the last" and "not in the last", a range and "before or after it". Numbers
are whole, so `>10` is "at least 11" exactly. A track missing the value (no year) matches neither
`year:<2000` nor `-year:<2000`, and neither `year:1994` nor `-year:1994` (read as "at most 1993
or at least 1995", since the editor's "is not" would take it in); never played is the opposite of any last-played
date. `-is:lossless` is lossy (a known lossy codec), as the editor's "Lossless: no" is.

**What is not understood** is searched as text, never refused: an unknown field (`re:stacks`),
a value that does not fit (`year:85`, `length:abc`), `has:lyrics` (lyrics are read from the files
only when shown, never kept, so search cannot tell), `composer:` (the library does not keep
composers), `rating:`, `is:rated` and `is:unrated` (ratings are no longer kept). A line under the search box says what ("Searched as text: “re:” is not a field"). A
colon with no field before it (`4:33`, "Mission: Impossible") is just text, and nothing is said.

**Results** keep their sections where they make sense: tracks are what the rules hold (24, then
"See all"), in the order of the first number or date asked about (most played first for
`plays:`, newest first for `added:` and `year:`), otherwise by artist and
album. Albums are listed only when every term can apply to an album (words, `album`,
`albumartist`, `artist`, `genre`, `label`, `year`, `added`, `is:favalbum`, `is:local`, `is:remote`): the
albums with a track that matches, words then looking in the album's title, album artist and label,
newest first. Artists only when every term is a word, `artist:` or `albumartist:` (no `OR`):
the artists whose names match. A play count or a length lists tracks only.

**Save as smart playlist** (a button above the results while the query has operators) opens the
smart playlist editor with the query's rules, in the same order, and the query as the name, not
saved yet: read the rules, change them, then Save. The **?** beside the search box opens a short
version of this table under it.

Search runs on its thread as before; only a query with operators takes the rules' path (reading
a query costs microseconds; a `playlist:` term asks the playlists table). Measured with
`search_time` on `search_fixture`'s 4,000 tracks (fast build, the development PC, median; 24 a section):

| Query | Path | Time |
| --- | --- | ---: |
| `beyonce` | plain | 1.7 ms |
| `kasane teto` (romaji, two passes) | plain | 3.5 ms |
| `artist:beyonce` | rules | 1.0 ms |
| `year:1990..1999` | rules | 1.2 ms |
| `beyonce is:local` | rules | 3.9 ms |
| `added:<30d` | rules | 0.2 ms |

`MP_BENCH=smart` times both paths on the messy library too (`bench-smart.txt`).

## Playlists

"Add to playlist" in a track's or an album's menu starts with "New playlist…" (also with no
playlists yet). It asks for a name in a small prompt where the menu was, beside what was
right-clicked: empty from a track, the album's title (selected, so typing replaces it) from an
album. Enter creates the playlist with what was right-clicked in one step, the pill says so
("Added 12 tracks to “Name”") and it appears in the sidebar; Esc, Cancel or a click outside
cancels. An empty name is refused in the prompt; a name a playlist already has (case and spaces
aside) is not made twice: the prompt offers to add to that one instead (its button, or Enter
again), and editing the name goes back to creating. Opened from the keyboard (Menu, Right on Add
to playlist, Enter), it works the same, and the list has the focus back when it closes. While it
is open it has the keyboard: no shortcut acts, Tab goes round its own controls.

Drag a row on a playlist's page to reorder it (Alt+↑/↓ from the keyboard), × or Delete
to remove it; rows chosen together move and go together (see [Selecting several tracks](#selecting-several-tracks)). Drop albums, tracks or queue rows on a playlist in the sidebar to add them
to its end; a drag reaching the left edge slides the sidebar in. Dropped on the sidebar's
Playlists heading or the empty space under the playlists, they make a new one (the name prompt,
at the drop point). Track menus there have
"Remove from playlist"; every track menu has "Show in Explorer" (a new Explorer window
with the file selected), and album menus show the album's folder. Track, queue and album
menus end with "View metadata" (see [Metadata](metadata.md#metadata)). The album page's header has the album's
menu too: right-click it, or the "⋯" button beside the heart.

A playlist's page has Play and Shuffle beside them, as an album's and a smart playlist's do,
then Rename, Export… and Delete; its menu (right-click it in the sidebar or on the Playlists
page) has Play, Shuffle, Add to queue, Open and the two exports. In the sidebar, pointing at a
playlist (ordinary or smart) shows a Play and a Shuffle button at its right, the menu's two
without opening it; a click on them does not open the playlist. Shuffle is the album's
(`playback::shuffle_tracks`): the queue becomes the playlist's tracks, shuffle turns on (the
player bar's mode, see `Queue`), and play starts from a random one; a track that cannot be
played, or whose file is gone, is skipped as in any queue. The page's button shuffles the
list it shows, the menu's the playlist as stored: the same tracks.

A name another playlist has (letter case and runs of spaces aside) is never taken silently.
Renaming a playlist, ordinary or smart, to it, or Create on the Playlists page with it, opens
the clash prompt in the middle of the window: what each holds ("“Road Trip” has 24 tracks.
This one (“Mix”) has 12 tracks."), **Keep both** (this one takes "Road Trip (2)", the first
number no playlist has; it has the keyboard, Enter) or **Cancel** (Esc, or a click outside:
the name stays as it was). There is no Replace here: that would delete the other playlist. A
rename to nothing leaves the name as it was; its own name in other letters is no clash. While
the prompt is open it has the keyboard, as the name prompt does (Tab goes round its controls).
`playlists/nameclash.rs`, `ClashPrompt` in `dialogs.slint`; playlist files ask the same way (see
[Playlist files](playlists.md#playlist-files)). "New playlist…" offers to add to the playlist of that name instead (above),
and the smart playlist editor refuses another playlist's name at Save.

## Selecting several tracks

Every track list (an album, an artist's top tracks, search results and their "See all", a
playlist, a smart playlist, the smart playlist editor's preview) and the queue (in its sidebar and
Up next) can have several rows chosen, as in a file manager (`selection.rs`, `selected.rs`):

- A click chooses a row (and gives the list the keyboard; its ring shows once a key is used
  there). A double click still plays it, as before. Ctrl+click adds a row or takes it away,
  Shift+click chooses the rows from the anchor (the last row clicked without Shift) to it,
  Ctrl+Shift+click adds that range. In the list that has the keyboard: Ctrl+A chooses every row,
  Esc none (Esc again does what it did: closes a sidebar, goes back), ↑ ↓ (and Home, End, Page
  Up/Down) move the selection with the cursor, and with Shift stretch it from the anchor.
- Chosen rows are drawn in the accent, stronger under the pointer; the playing track keeps its
  own marks (its accent title, the play glyph, the queue's fainter accent row).
- A right click (or Menu) on a chosen row opens the menu for all of them, under a caption saying
  how many ("12 tracks"): Play (these tracks, as the queue; in the queue: from the first), Play next,
  Add to queue, Add to playlist (an existing one, or "New playlist…" with them), Remove from
  playlist (a playlist's page), Remove from queue (the queue), Add to / Remove from favourites
  (Remove when every one is a favourite), and View metadata shown but not for several (it shows one track). A right click on a row that
  is not chosen chooses it alone and opens its own menu, as before.
- Each acts once on all of them, in list order, in one database transaction where it writes, and
  the pill says it once: "Added 12 tracks to “Mix”", "Removed 3 tracks
  from the queue". Files from outside the library (the queue can hold them) cannot be in a
  playlist or carry a favourite: they are left out and counted ("Added 10 tracks to
  “Mix”, 2 skipped: not in the library"). Tracks that play from the server (the cloud) are chosen
  and acted on like any other: none of these needs a file here.
- Dragging a chosen row drags them all, in list order: onto a playlist in the sidebar, onto its
  Playlists heading (a new playlist), into the queue. Within a playlist or the queue they move as
  one block, keeping their order, to the line the drop shows, and stay chosen there.
- Delete removes the chosen rows from the playlist or the queue (with none chosen, the row at the
  cursor, as before); never anything from the library or the disk. In a smart playlist it says
  why it cannot, as before.
- The selection belongs to one list: choosing in one clears any other, and another page clears
  it. When a list's tracks change (a scan, a smart playlist working itself out to something else,
  a track added to the open playlist), its selection goes; the same tracks in the same order keep
  it. The queue keeps the chosen items that are still in it, wherever they moved. Rows are known
  by a key of their own, never by their place alone, and a menu acts only if the rows it was
  opened on are still exactly the ones chosen: otherwise it does nothing and says so ("Those
  tracks are no longer chosen: nothing was done").
- A click changes a flag per row that changed and redraws those rows; nothing is rebuilt.
  Choosing all 2,550 tracks of the bench's playlist takes under a millisecond in the handler, and
  nothing runs while the app is idle.

## Smart playlists

A smart playlist is a playlist defined by rules (`library::smart`; the app's side is `playlists/smart.rs`
and `playlists/smartedit.rs`, `ui/smart.slint`). It always holds the tracks that match now.

**Rules.** Conditions joined by "all of" or "any of", each a field, an operator and a value. A
condition can be a group with the other join ("lossless and *any of* favourite, played 10+
times"); the editor makes groups one level deep (the format and the evaluation take any depth,
so a later editor can go deeper without a format change). Fields and operators:

| Kind | Fields | Operators |
| --- | --- | --- |
| Text | title, album, file path (either slash) | is, is not, contains, does not contain, starts with |
| Picked or typed | artist (every credited artist, featured too), album artist, genre, label, format | the same; "is" / "is not" pick from what the library holds (type-to-filter), the others take typed text |
| Number | year (the album's date), play count, plays in the last N days / weeks / months / years, length (whole seconds, typed `3:30`), bitrate (kbps), sample rate (typed in kHz), bit depth, disc number (none is disc 1), track number | is, is not, is at least, is at most, is between (inclusive) |
| Date | date added (the album's), last played | is in the last N days / weeks / months / years, is not in the last…, is before, is after, is between; dates typed `2024-03-15`, `2024-03` or `2024` (a month or a year stands for all of it), in local time |
| Yes / no | favourite, favourite album, played (at least once / never), lossless (yes / no: lossy), from the server (a track only the Navidrome server has; no: a file here) | |
| Playlist | another playlist, ordinary or smart | is in, is not in |

Text matches fold case, width, kana and Latin accents as search does (`fold.rs`): "beyonce" is
"Beyoncé", "よるしか" is "ヨルシカ". Several artists or genres: a condition matches when any value
does; "is not" and "does not contain" when none does (a track with no genre is not Jazz). A
missing value (no label, no year) matches "is not" and nothing else. "Not in the last 6 months"
includes tracks never played. Days, weeks, months and years count back on the local calendar
from now, to the same local time of day, and a typed date's day starts at its own local
midnight: across a daylight-saving change, "the last 7 days" is 7 days and an hour less or
more, and a winter date seen from summer is not an hour off (the system's time zone rules for
each instant, not today's offset).

**Order and limit.** By artist and album (the Albums page's order, an album's tracks in their
order), title, artist, album, year, date added, last played, play count, plays in a recent
period, length, or random; each either way round (missing values last). An optional
limit by tracks, minutes or hours, chosen by an order of its own ("25 tracks chosen by most
played", "2 hours chosen at random"; whole tracks, never past the total), then shown in the
list's order. Random is a stable shuffle by a seed kept with the playlist: the same order every
time, until **Reshuffle** (page button) picks a new seed. Not per day: a list that reshuffles at
midnight under a listener is a surprise, and a stored seed is the same after a restart.

**Live.** The library's generation moves on whenever what rules look at changes: a scan that
changed the library, a favourite (track or album), a play counted, a playlist's tracks
or name. Then what is on screen is worked out again: the open smart playlist, the Playlists
page's counts, the editor's preview; a home shelf when the home page is next shown. The page
keeps its rows (cursor, scroll) when nothing in it changed. Playing from a smart playlist copies
its tracks into the queue, as from any list, and the queue never changes with it: a "never
played" list loses the track once its play counts, the queue plays on. Nothing is re-evaluated on
a timer: with the app idle nothing runs.

**Evaluation.** One SQL query (`smart/sql.rs`), its values bound as parameters; the SQL text is
made only of the compiler's own fragments, so typed text can never change it. Multi-valued
fields and favourites are non-correlated `IN (…)` subqueries; plays, last played and plays in a
period read the `plays_track_time` index (schema 9). A smart playlist the rules name (at any depth)
is evaluated once, on its own, and each rule naming it looks in its ids (bound as one JSON array,
`json_each`), so the work grows with the playlists named, not with the ways to reach them (a
`WITH` would not do: SQLite expands one afresh at every reference); a cycle (or nesting deeper than 8) is refused in the editor (its list of
playlists leaves out those that would come back) and, if one is stored anyway, that rule matches
nothing and the page says so. All evaluation runs on a thread of its own with its own read
connection (`mp-smart`): the page, the preview (150 ms after the last change, the newest asked
wins), counts, home shelves, and Play / Add to queue / Export from a menu. Measured with
`smart_time` on `smart_fixture`'s 20,000 tracks and 97,964 plays (release build, the development PC;
median of 14 after a first run):

| Rule set | Tracks | All rows | Count |
| --- | ---: | ---: | ---: |
| Favourites not heard in 6 months | 944 | 3.5 ms | 2.4 ms |
| Lossless only | 7,240 | 9.1 ms | 2.3 ms |
| Most played this year (50, by plays in 12 months) | 50 | 9.2 ms | 10.7 ms |
| Recently added and unplayed | 1,971 | 4.1 ms | 2.0 ms |
| Two hours at random | 26 | 12.7 ms | 13.3 ms |
| Title contains "love" | 1,240 | 5.6 ms | 4.0 ms |
| Genre is not jazz, by year | 17,640 | 27.6 ms | 5.7 ms |
| Not played in 6 months, by last played | 11,447 | 22.2 ms | 6.8 ms |
| Every track, by artist and album | 20,000 | 32.4 ms | 1.7 ms |
| Any of ten mixed conditions, random | 11,717 | 35.9 ms | 17.4 ms |

(The slowest is reading 20,000 rows of a whole list, what an ordinary playlist of that length
costs.) `library::smart::tests` checks every field with every operator, relative dates on a fixed
clock, limits, orders, the shuffle, cycles, the JSON form, unknown rules and hostile text, and
3,000 random rule sets against an in-memory evaluation written apart from the SQL
(`smart/reference.rs`), members and order.

**Where they show.** Beside ordinary playlists: the sidebar (a filter glyph; no drop target), the
Playlists page (their rules in words, "Genre is Jazz · lossless · 50 tracks, most played", and
how many tracks, counted off the UI thread), home shelves (Settings > Home > Edit shelves, "By
playlist"), and their own page: the rules in words, the count, Play, Shuffle, **Edit rules**,
Reshuffle (random only), "⋯" (Rename, Duplicate…, Save as ordinary playlist…, Export) and Delete.
The list is a track list like any other (play, queue, favourite, drag rows out to the queue
or a playlist), but its contents and order are the rules': no drag reorder, no × on rows, no drops
onto it; Delete or Alt+↑/↓ on a row says so in the pill ("edit the rules to change what is in
it"). "Add to playlist" menus and the name prompt's "add to existing" offer ordinary playlists
only. Save as ordinary playlist opens the name prompt with "<name> (saved)" and makes an ordinary
playlist of what the list holds now, in its order; Export writes what it holds now. A playlist
file named like a smart playlist asks first (see [Playlist files](playlists.md#playlist-files)): Keep both or Cancel, never Replace. Deleting a playlist that
smart playlists name in their rules: Delete's first click, which arms it, names them ("Click again: “Best of”
uses it"; `query::smart_playlists_using`), as their rule about it will then match nothing. A home
shelf made from a playlist is titled with the playlist's name (as it is now) unless given its own.

**The editor** (a page; Back and Forward keep its draft). A new one starts on its name, then the
rules; starting points ("Recently added and unplayed", "Favourites not heard in 6 months", "Most
played this year", "Lossless only", "Two hours at random") fill the draft, to be
changed. Each line is controls that read as a sentence: a choice opens its list under it (Enter,
Down, F4 or a click; typing filters long lists; Up, Down, Enter, Esc), a typed value is a field; up, down and × on each line, "Add rule", "Add group", a group's own
"Rule". What a line cannot be yet is said under it (once it was touched, or on Save); the line
above the rules says what they match now ("Matches 142 tracks · 9 h 12 min") or what to fix
first, and the preview below lists the first 100. Save is refused, with the reason and the
keyboard on the value at fault, while a rule is incomplete, the name is empty or another
playlist's, or a rule would bring the playlist into itself. Cancel (or Esc) leaves; Duplicate
makes an unsaved copy "<name> copy".

**Storage.** `playlists.rules` (schema 9): NULL for an ordinary playlist, the rule set as JSON
for a smart one, which has no `playlist_items`:

    {"match":"all","rules":[{"field":"genre","op":"is","value":"Jazz"},
      {"group":"any","rules":[{"field":"favourite","op":"yes"},{"field":"plays","op":"at_least","value":10}]}],
     "sort":{"by":"plays","desc":true},"limit":{"n":50,"unit":"tracks","by":{"by":"random","desc":false}},
     "seed":1234,"v":1}

Versioned (`v`). A field, operator, join, order or limit this build does not know (a newer
build's, or damaged) is kept exactly as stored and written back so; it never crashes and never
widens the result: an unknown condition matches nothing (in "any of", the others still count),
an unknown limit shows nothing rather than too much, an unknown order shows by artist and album;
the page and the editor say which ("A rule from a newer version of Marquee Player (the field
“mood”)…"). Saving never lowers the version.

**Ratings, from an earlier version.** The app no longer keeps ratings. A smart playlist saved
with rules about them is written again when the app starts (`query::forget_rating_rules`,
logged per playlist; a second start finds nothing more to do), and no rule about ratings ever
widens what a list holds or turns it into every track:

- Among other rules of "all of", a rating condition (or a group of nothing but rating
  conditions) is dropped and the others kept: "Jazz, rated 4+" becomes "Jazz".
- Anywhere else it stays, as a rule this version cannot use: it matches nothing, and the page
  and the editor say "A rule about ratings, which are no longer kept". A list that was only
  "rated 4+" (or "rated 2+ and at most 4") is then empty, never the whole library; in "any of"
  the other choices still count ("Jazz, or rated 5" holds the Jazz); a group of nothing but
  ratings matches nothing. Edit the list to take the rule out, or delete it.
- An order by rating, or a limit chosen by rating, becomes the default order (the limit keeps
  its size).

A list that names one of these ("is in Top rated") holds only what the named list's other
rules match, and says so; "is not in" a list with a rule that matches nothing in place of what
it would (a rating, a rule from a newer version, a playlist that is gone, comes back or is too
deep) matches nothing too, rather than more than it should. A list an earlier start of this
version already emptied (no rules at all) is left alone: it cannot be told from one made with no
rules. Read from a file in any other way (`Smart::from_json`), the same rules are read the same
way.

## Keys

Settings > Keys lists every remappable action with its key. Click a key (or Tab to it and
press Enter) and press the new one: Esc cancels, Delete (or "No key") leaves the action
without a key, and the circular arrow puts the row's default back; "Reset all keys" resets
everything. A key another action has is never taken silently: the editor names that action
and asks, Reassign (or Enter) gives the key to the new action and leaves the other one
without it, Cancel (or Esc) keeps both as they were. Resetting a row takes its default keys
back the same way and says which action lost one. The fixed keys below are refused, with
the reason. Every change is saved at once: `settings.json` keeps only the actions whose keys
differ from the defaults (`"shortcuts": { "mute": ["K"], "repeat": [""] }`, `""` for none), so
a future change of a default still reaches anyone who did not remap that action, and a key
you chose always wins over a default (the action that had it by default goes without). An
entry that cannot be read is ignored with a warning in the log, never the whole file;
the keys an earlier version saved for rating (`rate_0` … `rate_5`) are dropped without one.
Shift and a letter is a key of its own (`Shift+V` is not `V`); pressed where no action has it,
it acts as the letter, as before it could be given (a typed character such as `/` or `?`
carries its Shift with it, whatever the layout).
Tooltips and screen-reader labels (the player bar, the pill, the pin buttons) name the
current key.

Remappable, with their default keys:

| Key | Does |
| --- | --- |
| `Space` | Play / pause |
| `←` / `→` | Seek 10 s |
| `Ctrl+←` / `Ctrl+→` | Previous / next track |
| `Ctrl+↑` / `Ctrl+↓` | Volume up / down 5% |
| `M` | Mute / unmute |
| `S` | Shuffle on / off |
| `R` | Repeat: off, all, one |
| `L` | Favourite the playing track, or unfavourite it |
| `N` | Now playing (again: back) |
| `Y` | Lyrics on now playing (again: Up next) |
| `V` | Visualizer on / off |
| `C` | The visualizer's next look (the built-in presets, then the user's), said in the pill; turns it on if off |
| `Shift+V` | Where the visualizer shows: behind the page, in the panel (its tab chosen), full page, and round; said in the pill (with "(on Now playing)" on another page, where it moves without showing). Off, it comes on where it was |
| `/` and `Ctrl+F` | Search (two keys: each can be changed or cleared on its own) |
| `Alt+←` / `Alt+→`, mouse side buttons | Back / forward (the side button nearer you is back; the buttons are not remappable) |
| `Ctrl+B` / `Ctrl+Q` | Pin or unpin the sidebar / the queue |
| `Ctrl+S` / `F2` | View metadata with tag editing on: review and save the edits / edit the field (or track row) at the cursor. They act nowhere else; in a field being edited only a Ctrl chord is taken |
| `Ctrl+C` / `Ctrl+V` | Settings > Equalizer: copy the setting as Equalizer APO text / paste one (shown before it applies). Settings > Visualizer: copy the look as a line of text / paste a preset (shown before it is added). They act nowhere else, and never in a text field, which keeps its own copy and paste |
| `G` / `H` | The playing track's synced lyrics 0.1 s earlier / later (hold for more; kept for the track; said in the pill). Plain lyrics: says they have no time stamps |
| `Ctrl+=` and `Ctrl++` / `Ctrl+-` / `Ctrl+0` | Interface size a step larger / smaller / back to 100% (said in the pill; see [Interface size](settings.md#interface-size)). `Ctrl++` is the keypad's + or a layout's own; on a US keyboard `Ctrl+Shift+=` is that too. The keypad's - is `Ctrl+-` |

Fixed:

| Key | Does |
| --- | --- |
| `Esc` | Clear the rows chosen in a list; else close a sidebar that slid in (by key, button or pointer), else back; cancel |
| `Tab` / `Shift+Tab` | Move between buttons, links, fields, sliders and lists |
| `Enter` | Press the focused button; in a list, open the album, artist or genre, or play the track |
| `↑ ↓ ← →`, `Home`, `End`, `Page Up/Down` | Move in a list or grid of covers (a home shelf: ← →; a track list: ↑ ↓, as ← → seek, and the selection moves with the cursor); on a focused slider, change it |
| `Shift+↑` / `Shift+↓`, `Shift+Home` / `End` / `Page Up/Down` | In a track list or the queue: stretch the selection from its anchor |
| `Ctrl+A` | In a track list or the queue: choose every row (in a text field: select all its text) |
| Click, `Ctrl`+click, `Shift`+click | In a track list or the queue: choose a row; add or take one away; choose the range from the anchor (`Ctrl+Shift`+click adds it) |
| `Menu` or `Shift+F10` | The menu of the focused album or track (of all the rows chosen, when it is one of them) |
| `Delete` | Remove the chosen tracks (else the focused one) from the queue, or from the open playlist (a smart playlist's: says why it cannot) |
| `Alt+↑` / `Alt+↓` | Move the focused track in the open playlist |
| `Ctrl+A`, `C`, `V`, `X`, `Z`, `Y` | Text editing in a text field (outside one, `Ctrl+A` chooses a list's rows, `Ctrl+C` and `Ctrl+V` are the equalizer's copy and paste, above) |
| `Ctrl+Shift+D` | Show the last side-button input received, for diagnosis |

A remap cannot take a fixed key: plain `↑ ↓ Home End Page Up/Down`, `Tab`, `Enter`, `Esc`,
`Menu`, `Shift+F10`, `Delete`, `Alt+↑/↓`, the text-editing chords (but `Ctrl+C` and `Ctrl+V`
for the equalizer's copy and paste, which act only outside text fields), `Alt+F4` and `Alt+Space`
(the window's), and any `Ctrl+Alt` chord (AltGr on many keyboards, which types characters).
Plain `←` and `→` can only be the seek keys (grids of covers move with them). Media keys go
through the Windows media overlay and are not part of the keymap.

A key bound to an action never acts while you type in a text field: the field keeps every
key it takes. The pin actions' keys are the exception when they are Ctrl or Alt chords on a
letter, digit, F-key or named key: the window takes those before the focused control, so
`Ctrl+B` and `Ctrl+Q` (or their replacements) work in the search field too. A plain key, or
a chord on a punctuation key (which moves between keyboard layouts), given to a pin action
works everywhere but in a text field. Elsewhere the keys act wherever the focus is. The
keyboard focus shows as an accent ring. A list or a grid takes the focus as one control and
moves a cursor over its items, so the focus survives rows being recycled as it scrolls.
Controls carry accessible roles and labels for screen readers (icon-only buttons included;
tracks and albums read their title and artist; a chosen row says it is selected). Right-click a
track or an album (or press Menu) for queue and playlist actions. Album titles, artist names and album names are links
wherever they appear.

## Middle-click scrolling

As in Chrome, Edge and Firefox: a middle click on something that scrolls leaves an anchor
mark there (arrows for the ways it can go, the one it is going lit) and shows the scroll
cursor; the content then scrolls toward the pointer, faster the further it is. Within
`[autoscroll] dead_zone` (15 px) of the anchor on an axis nothing moves; past it the speed is
`speed` px/s (600) at 100 px beyond, growing with the distance to the power `exponent`
(1.8, so gentle near the anchor and quick far away), up to `max_speed`. It runs on the frame
clock in pixels per second, so it is the same speed at any refresh rate, and it stops at the
content's ends. Any click, the wheel or Esc ends it (and does nothing else), as does a page
change, another Settings section, the other tab of now playing's panel, or the window losing
focus. Held: press, drag and release ends it on release if the pointer left the dead zone;
a click without moving leaves it on. A middle click never opens, drags or moves anything,
and one on something that does not scroll does nothing. Settings > Window > Middle-click
scrolling turns it off (the middle button then passes through as before); with a menu open
the middle button goes to the menu.

Every scrolling area takes part: the pages, the album grids, track lists and playlists,
the queue (sidebar and now playing), the sidebar's playlists, Settings' column and its
section strip, the lyrics, the shelf editor, and the horizontal home shelves and chip rows.
Under the anchor, each axis goes to the innermost area that can scroll on it: a home shelf
takes the horizontal axis and the page around it the vertical one (the cursor shows four
ways), as browsers do. Panels over the page (the sidebars, the player bar, the pill) take a
middle press for themselves, so it never scrolls the page under them. The scrolled area
stays open while it scrolls (a sidebar that slid in), and the lyrics stop following the
sung line as for a manual scroll. Nothing runs while the mode is off, in the dead zone or
at an end. The input is taken in winit's event hook, before Slint sees it
(`autoscroll::install`); each area claims a press in `ui/autoscroll.slint`.

## Tray icon

While music plays, the tray icon's five bars bob like an equalizer, each on its own two slow
rhythms (`[tray] fps` = 5 frames a second); it does not read the audio. When playback
pauses, stops or ends, the bars ease back to the static glyph over 0.6 s and the timer stops.
Settings > Window > Animated tray icon turns it off (the icon is then always static).
On macOS the menu bar's status item is always the static glyph and Settings has no switch for
it: Slint's AppKit tray turns every frame into a PNG and back before AppKit redraws the menu bar
(`platform::system::TRAY_ANIMATES`).
Each frame is drawn at the notification area's icon size and handed to Slint's tray, which
replaces the icon (`NIM_MODIFY`) and frees the old one. Measured (`MP_BENCH=tray`, window
hidden, playing): 0.35% of one core with the animation off, 0.45% with it on; a frame takes
0.05 ms; no GDI or USER objects leaked over the run; paused and hidden, idle as before. The
shell (explorer.exe) also redraws the icon on every frame; at 12 fps that was roughly 1% of a
core in explorer (noisy to measure), less at 5.

The mouse on the icon (`tray/mod.rs`):

| On the icon | Does |
|---|---|
| Click | Play or pause; with nothing playing, starts the queue as the Play button does |
| Double click | Show the window (unminimized, in front), or hide it to the tray if it is shown |
| Middle click | Next track |
| Wheel | Volume, 5% a notch (the volume keys' step); the tooltip leads with "Volume 45%" for 3 s |
| Right click | The menu, as before: Play/Pause, Next, Previous, Show Marquee Player, About, Quit |

A click waits the system's double-click time (Control Panel > Mouse; 500 ms by default)
before it plays or pauses, so a double click never also plays or pauses: nothing to undo and
no blip of sound, at the price of that wait. Settings > Window > "Click on the tray icon"
brings back the old click: on "Show window" a click shows the window at once (and a double
click adds nothing); the middle button, the wheel and the menu stay. "Hide" is the close
button's hide to the tray, whatever the close button is set to.

On Windows, Slint's icon only says "clicked" (a left release) and opens its menu on a right
one, so `platform/windows/tray.rs` subclasses the hidden message-only window Slint's icon
reports to (class `SlintSystemTrayWindow`, found once the event loop has made it): the left
release, `WM_LBUTTONDBLCLK` and the middle release are the app's and never reach Slint; the
right button and the rest go on to Slint. `Shell_NotifyIcon` sends no wheel messages, so
while the pointer is over the icon a low-level mouse hook (`WH_MOUSE_LL`) is put in: the
icon's `WM_MOUSEMOVE` arms it, and only if the cursor is inside the rectangle
`Shell_NotifyIconGetRect` gives (the message may be late). It comes out at the first
movement outside that rectangle, when a check four times a second finds the cursor outside,
after 5 s with neither movement nor wheel over the icon, when the window goes, at exit and in
the crash handler. It lives on a thread of its own that waits in `GetMessageW`, so a busy UI
thread never holds up the system's mouse input; its callback compares the point, posts a
message for a wheel notch and passes every event on (the wheel is never taken from what is
under the pointer). In the overflow flyout it works where the shell gives the icon's
rectangle there, and otherwise there is simply no wheel. Nothing runs, and there is no hook,
while the pointer is anywhere else. Measured: the `tray` bench ([Measurement and test hooks](../testing.md#measurement-and-test-hooks)) also hands the icon's
window the shell's messages for each gesture inside the app (no real input) and checks the
mapping, and that the hook is in only if the real pointer is over the icon; the wheel itself
over a real icon was not tried by the harness.

## Opening files from outside

Audio files or folders passed on the command line play at once: tracks in the library as
library tracks, others as plain files whose tags are read on the fly, never added to the
library (nothing is recorded for them: plays, favourites). A second launch with
files hands them to the running instance (`WM_COPYDATA`) and exits. Files and folders
dropped from Explorer on the window play now; dropped on the queue (the queue sidebar, or Up
next on now playing) they are added to its end. The installed edition is in "Open with" for
every format, and Default apps can make it the default (see [Editions](../releases.md#editions)); the portable and
standalone exes register nothing. Playlist files (.m3u, .m3u8) dropped on the window are
imported as playlists, not played (see [Playlist files](playlists.md#playlist-files)).
Drops arrive as `WM_DROPFILES` (winit's own drop target is turned off: it reports no drop
point, and Slint does not pass external drops to `DropArea`), so there is no drop
highlight while dragging.
