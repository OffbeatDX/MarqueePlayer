# Scrobbling and Discord

Last.fm, ListenBrainz and Discord's "Listening to".

## Last.fm

Settings > Connections > Last.fm scrobbles plays with your own API account: paste its API key and shared
secret (create one at last.fm/api/account/create) and press Connect. The app gets a token,
opens Last.fm's authorisation page in the browser, and connects as soon as access is
allowed; Settings then shows the user name, Disconnect and Send now. The key, secret and
session key are kept in `lastfm.json` in the data folder (never in the database, never in
the log), the shared secret and session key sealed as the server's token in `navidrome.json`
is (DPAPI for the Windows user; on Linux the file is the user's alone, mode 0600). A file
from an older version, with the secret or session key in plain text, is written again sealed
when read; a secret or session that does not open here (the portable edition on another PC,
another Windows account, damage) is cleared: the API key stays, and re-entering the secret
and pressing Connect signs in again.

"Now playing" goes out when a track starts. A track longer than 30 s is scrobbled once it
has played half its length or four minutes, whichever comes first (Last.fm's rule; the
app's own play count keeps its 75% rule), with the time it started, its artist (as tagged;
see [Artists, albums and imperfect tags](library.md#artists-albums-and-imperfect-tags), Online services), title,
album, album artist, length, track number and, when tagged, its MusicBrainz recording id
(read from the file then; the library does not index it). Scrobbles are written to
`scrobbles.json` before anything is sent, so none is lost to a crash or exit; they go out in
batches of up to 50, oldest first. Offline or a Last.fm outage: they wait and are retried
(30 s, doubling to 30 min, or at once with Send now), also after a restart. A play Last.fm
ignores or rejects for good is dropped and logged. An invalid session stops sending and
asks to connect again; a refused key or secret asks to check them. All network work runs on
its own thread (`crates/lastfm`, WinHTTP); playback and exit never wait for it.
`crates/lastfm/tests/mock.rs` runs the whole flow against a local mock server. Tracks of a
Navidrome server can be left out of the app's scrobbles (Settings > Library > Server, "Scrobble
server tracks to Last.fm from this app"; see "What goes back to the server", under [Navidrome](navidrome.md#navidrome)),
for a server that scrobbles them itself.

## ListenBrainz

Settings > Connections > ListenBrainz sends what you play to a ListenBrainz profile, beside
Last.fm and independent of it: either, both or neither may be connected. Paste the user token
from your ListenBrainz settings page ("Open the token page" opens it in the browser, through the
app's own way of opening a link) and press Connect: the token is checked (`GET
/1/validate-token`) and the card shows the user it belongs to; Disconnect forgets it. The field is
emptied once connected and the token is never shown again, logged, or put in Copy diagnostics: it
is kept in `listenbrainz.json` in the data folder, sealed as Last.fm's session key and the
server's token are (DPAPI for the Windows user; on Linux the file is the user's alone, mode
0600). A token that does not open here (the portable edition on another PC, another account) is
no token: connect again. **Server address** (empty: `https://api.listenbrainz.org`) sends to a
server of one's own that speaks ListenBrainz's API instead (the address of that API, `/1/...`
added to it); plain `http://` is allowed, with the note that the connection is not encrypted,
and the token page is not offered for it.

The plays it is sent are decided where Last.fm's are (`connections/scrobble.rs`, one decision for both, so the
two cannot drift apart): "now playing" (`playing_now`) when a track starts, and the listen once it
has played half its length or four minutes, whichever comes first, for a track longer than 30 s
(the rule ListenBrainz documents too), with `listened_at` the time it started (Unix seconds, UTC).
A listen carries the artist (as Last.fm gets it), the title, the album as `release_name`,
`duration_ms` and `tracknumber`, and in `additional_info` the MusicBrainz ids it is tagged with:
`recording_mbid`, `track_mbid` (the release track), `release_mbid`, `release_group_mbid` and
`artist_mbids`, read from the file on the sending thread before it is first sent (a track only the
server has: the ids the server gave); only well-formed ids are sent. `media_player` and
`submission_client` are "Marquee Player", with the version in `..._version`. Nothing names the
Navidrome server: the documentation's `music_service` is a public service's domain, which a
server of one's own is not, so it is left out.

Listens are written to `listens.json` before anything is sent, so none is lost to a crash or
exit, and go out oldest first: one as `single`, several as one `import` of at most 1,000 and 10 MB
(the documented limits). Offline, a 5xx or an unexpected answer (an address that is not a
ListenBrainz API): they wait and are tried again (30 s, doubling to 30 min, or at once with Send
now), also after a restart. Requests are at least a second apart (the documented one call a
second); `X-RateLimit-Remaining` and `X-RateLimit-Reset-In` are honoured, and a 429 waits as long
as it says, then sends again. A 400 for a batch halves it until the listen it refuses is alone;
that one is dropped and logged, as is a listen that can never be sent (no artist or title, a time
before ListenBrainz's minimum, over 10 KB). A token no longer accepted (401) stops sending and asks
to connect again (the section is marked); the queue stays and goes once connected again.

**No duplicates.** A listen leaves the queue only once ListenBrainz answered 200 for it. Its ids are
read in and written to the queue before it is first sent, so a listen sent again because its
answer was lost (a timeout, a dropped connection) is the same request byte for byte: the same
user, time and metadata, which ListenBrainz stores once (its listen store keeps one listen per
user, time and recording; this is the server's behaviour, not a promise of the API
documentation). One play is queued once, and a listen already queued (the same time, artist and
title) is not queued again.

**Love favourites on ListenBrainz** (off by default): a track made a favourite here is loved
there (`POST /1/feedback/recording-feedback`, score 1), and no longer loved (0) when its heart is
taken away, when it has a MusicBrainz recording id (the scan's, the file's or the server's). Only
hearts given from now on; one given while ListenBrainz cannot be reached is sent while the app
stays open (not kept on disk).

All network work runs on its own thread (`crates/listenbrainz`, WinHTTP), never the UI's; playback,
exit and Last.fm never wait for it, nor it for Last.fm. `crates/listenbrainz/tests/mock.rs` runs
the whole flow against the stand-in (`listenbrainz::mock`), `connections/scrobble.rs`'s tests both services at
once with one of them down or hanging, and `MP_BENCH=listenbrainz` the card in the app. Tracks of a
Navidrome server can be left out, for a server that scrobbles them itself (Settings > Library >
Server, "Scrobble server tracks from this app", which covers both services).

## Discord

Settings > Connections > Discord ("Show on Discord", off unless turned on) shows the playing
track on the user's Discord profile as "Listening to": the title, the artist as tagged, and
how far in it is. It presents Marquee Player's own Discord application (`presence::APP_ID`,
built in; an application ID is not a secret). To appear under another name, the ID of a
Discord application of the user's own (created at discord.com/developers/applications) goes
in Settings > Application ID and is kept in `settings.json` (`discord`): an application's name
is what Discord shows after "Listening to", its icon the picture.

Nothing goes over the network from the app: `crates/discord-ipc` talks to the Discord app
running on the same machine over its IPC pipe (`\\.\pipe\discord-ipc-0` to `-9`; a Unix socket
of that name in the runtime folder on Linux, also where Flatpak and Snap put it), one thread,
never joined. The activity is set while a track plays, again after a seek (the start time
moves by more than 2.5 s), and cleared on pause and stop; closing the pipe (the setting turned
off, the app exiting) clears it too. Discord takes five updates in twenty seconds: only the
latest activity wanted is sent, at most four in twenty seconds, so skipping through tracks
shows the one that stays. Discord not running: looked for every 15 s, and what plays is shown
once it answers. Discord quitting is noticed at the next activity sent (the next track, a
pause, a seek). An ID that is not a number, or one Discord does not know (close code 4000), is
said in Settings, marks the section, and is not tried again until it changes. Covers are not
shown: Discord takes pictures only by URL or from the application's own uploaded assets.
`crates/discord-ipc/tests/mock.rs` runs the flow against a mock of Discord over a loopback
socket; a bench run never opens the pipe.
