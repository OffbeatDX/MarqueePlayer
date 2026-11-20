# Navidrome

Settings > Library > Server adds the music on a Navidrome server (or another Subsonic or
OpenSubsonic server) to the library, beside the music folders: folders only (as before), the
server only, or both as one library, each album and track listed once. It is the app's own
library all the same: favourites, ratings, plays and playlists are the app's, as they are for
the folders' music. The server's music files are never changed. What the app writes back is
your favourites and ratings and your plays, each behind its own switch (see "What goes back to
the server"); those calls (`star`, `unstar`, `setRating`, `scrobble`) change only the server's own
database. Nothing else is ever sent: no playlist, no delete, no tag, no scan. The sync, the
covers and streaming use a read-only client (`subsonic::Client::new(.., true)`).

**Signing in.** The address the server opens at in a browser (`http://` is allowed, with a note
that the connection is not encrypted; fine on a home network), the user and the password, then
Connect. The password makes the token that signs every request and is dropped at once: it is
never kept, and neither the address nor the user is written to the log. The salt and token are
kept in `navidrome.json` (see [Files](../architecture.md#files)), sealed with DPAPI for the current Windows user (on Linux
the file is the user's alone, mode 0600). The portable edition's drive taken to another PC (or
another Windows account) carries a sign-in that cannot be opened there: Settings asks to sign in
again, and the server's music stays in the library meanwhile. A wrong address, no answer,
something that is not a Subsonic server, a refused password or an account that cannot use token
sign-in (LDAP) are each said in their own words under the fields.

**One library** (`library::remote`, whose module documentation has every rule):
- a track the server has and a file also has is one row: the file's, with its tags (local tags
  win; the server never changes what a file says, or a local album's dates);
- the strongest sign that the server's song is the folder's file is the file itself: the same
  size and a path ending in the same folder and file name (case and separators aside). Then
  MusicBrainz ids, then album artist and title with the year and the track list agreeing. An
  album whose matched files are all one local album's is that album, even with a title spelled
  differently. When in doubt, two rows: a wrong merge would hand one album's history to another;
- a track only the server has is a row of its own, shown, searched, sorted and grouped as any
  other (its album dated by when the server first had it, for Recently added);
- when a file is deleted and the server has it, the track stays, as the server's (its plays,
  favourite, rating and playlist places with it); a file that appears for a server track becomes
  one row with it (after every scan: `remote::reconcile`).

**Syncing** (`server/sync.rs`): at start (a few seconds after the window is up), after signing in,
on Sync now, and when the window comes back from the tray after three hours or more away; never
on a timer. Each first asks the server whether it has scanned since the last sync that saw
everything, and lists nothing when it has not (sign-in and Sync now list anyway). A listing is
the album list, 500 a page, then each album with its songs, three at a time, written every 60
albums: the first albums are on Home within seconds of signing in, and open pages take them in
as they do a scan's batches (also on a Home that was empty: it fills while the sync runs). Progress
shows at the foot of the sidebar, in Settings and in the top right of Home, as a scan's does ("What
it says", under [Scanning](library.md#scanning); on an empty home, in its large words), and Stop keeps what was written
(the next sync goes on from there). A scan and a sync take turns writing
the library. Only a listing that read every page and every album without an error removes what
the server no longer lists; when that is most of it at once (a server missing a drive: at least
25 tracks, fewer than one in ten left), Settings says so and removes them only when asked. A
server that does not answer is a quiet line in Settings and the library stays as last synced; a
sign-in the server no longer takes marks Settings' Library section and asks to sign in again.

**Covers.** An album only the server has gets its cover from the server, into the art cache
(the same sizes, backdrop and accent as a local cover; see [Art cache](library.md#art-cache)): newest albums first,
three at a time, after each sync, and first of all for a tile on screen without one. An album
with a file of its own keeps the art the scan found. Each album records which cover it fetched in
which cache format (`albums.art_checked`), so a pass interrupted goes on where it was, a cover
the server does not have is not asked for again, a cover changed on the server is fetched again,
and a new cache format fetches every server cover again once, newest first, the old images
showing meanwhile.

**Marked with a cloud.** What plays from the server (a track only the server has, an album none of
whose tracks has a file here) has a small cloud after its title: album tiles (Albums, Home's
shelves, an artist's page, search), the album page's header, Home's featured album, and every
track list (album, playlist, smart playlist, search, the queue and Up next). A local track the
server also has shows nothing: it plays from its file. Hovering the cloud names the server, its
address without the scheme (`https://music.example.org/nd` as `music.example.org/nd`;
`Server.host`, one property for every row, also while signed out with its music still shown); a
screen reader hears "from the server". The cloud takes no click and changes no row's height.

**Connect Navidrome** on the empty home page (shown while no server is signed in) opens Settings >
Library with the sign-in card in view and the address field focused.

**What a track only on the server cannot do** (there is no file here): Show in Explorer is not
offered for it or its album; View metadata shows what the library knows, "From the server",
instead of the file's tags, and offers no editing (it names the server's file, and for an album
only the server has, the server's album and no folder, never a file or folder here); an M3U export leaves it out and says how many
it left out; Last.fm gets its plays without a recording id read from a file, and ListenBrainz with the ids the server gave (its recording, artists and release). Settings > About
counts the tracks only in the folders, only on the server and in both; Copy diagnostics names
the server software and those counts, never its address or the user.

**Signing out** asks nothing: the server's music leaves the library (tracks only it had go,
with their plays; the folders' tracks only forget the server), the sign-in is forgotten, and the
folders' music is as it was. Signing in to another server (or as another user) replaces the
first one's music the same way.

**What goes back to the server** (`app/src/server/write.rs`, `library::marks`). Three switches,
below the sign-in, all on by default:
- **Sync favourites and ratings with the server**: both ways, for every track the server has
  (its own tracks, and files matched to its songs). A heart or stars given here go to the server
  at once, off the UI thread; given there, they come here at the next sync. A sync that lists
  nothing (the server has not scanned) still reads the server's stars (`getStarred2`, one
  request); a rating changed there comes with the next listing (after a scan there, or Sync
  now). Per song, the favourite and rating last agreed with the server are kept
  (`remote_marks`, schema 13), so each sync tells which side changed: only the server's, it is
  taken; only here, it is sent; both (to different values), a rating made here wins, and for a
  favourite the later change wins when both times are known (the server's star time, the app's
  own change time), else the one made here. The first sync after this came (nothing agreed yet)
  keeps every favourite of either side (starred on one, starred on both) and a rating made here,
  else takes the server's. The scale is the same on both sides, 1 to 5 whole stars and none
  (`setRating` 0), so nothing is rounded: a value sent and read back is the value sent, and a
  value no side changed is never written. Offline, a change waits: a value here that differs
  from the agreed one is what is still to send, so the next sync sends it, also after a restart.
  Album favourites stay the app's own.
- **Report plays to the server**: each play counted here of a track the server has is reported
  (`scrobble`, with the time it was counted), and "now playing" when one starts. The play is
  queued in the library with the play itself (`remote_plays`, one transaction), sent oldest first,
  and off the queue once the server took it (or no longer has the song); offline, it waits for
  the next sync. Turned off, the plays not yet sent are forgotten. The server's own play counts
  are not taken into the library: the app's plays are what it reports, so they would count
  twice.
- **Scrobble server tracks from this app** (Last.fm and ListenBrainz alike; shown greyed with a
  word while neither is connected; see [Last.fm](scrobbling.md#lastfm) and [ListenBrainz](scrobbling.md#listenbrainz)): off, the app's own now
  playing and scrobble or listen are skipped, for both services, for a track played from the
  server, and for a file the server also has while plays are reported to it (the server hears of
  that play too). For a server that scrobbles itself (Navidrome can, to Last.fm and to
  ListenBrainz), so plays are not counted twice; the plays switch's hint points at it. A track the
  server does not have is always scrobbled. (Until ListenBrainz came this switch was Last.fm's
  alone, "Scrobble server tracks to Last.fm from this app"; its value in `settings.json`,
  `server_lastfm`, is kept and now covers both.)

With the first two off, the client may send no write at all (`subsonic::Writes::NONE`: the
guard refuses each before a request is made). The writes run on one thread of their own
(`server/write.rs`), the only one with a client that may write, so a change made here and a sync
never cross. Signing out forgets what was agreed and the plays not yet sent.

**Playing from the server** (the stream cache, loading, prefetch, lyrics from the server): see
[Streaming](lyrics.md#streaming), under [Lyrics](lyrics.md#lyrics).
