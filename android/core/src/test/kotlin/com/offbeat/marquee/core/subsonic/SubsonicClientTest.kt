package com.offbeat.marquee.core.subsonic

import kotlinx.coroutines.test.runTest
import okhttp3.HttpUrl.Companion.toHttpUrl
import okhttp3.Interceptor
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Protocol
import okhttp3.Request
import okhttp3.Response
import okhttp3.ResponseBody.Companion.toResponseBody
import okhttp3.mockwebserver.MockResponse
import okhttp3.mockwebserver.MockWebServer
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Before
import org.junit.Test
import java.net.ProtocolException
import java.util.concurrent.TimeUnit

class SubsonicClientTest {
    private lateinit var server: MockWebServer

    @Before
    fun setUp() {
        server = MockWebServer()
        server.start()
    }

    @After
    fun tearDown() {
        runCatching { server.shutdown() }
    }

    private fun client(readOnly: Boolean = true, path: String = "/") = SubsonicClient(
        SubsonicCredentials(server.url(path).toString(), "alice", "c19b2d", "26719a1196d2a940705a59634eb18eab"),
        readOnly = readOnly,
        httpClient = OkHttpClient.Builder().readTimeout(5, TimeUnit.SECONDS).build(),
    )

    private fun ok(body: String = "") = MockResponse().setBody(
        """{"subsonic-response":{"status":"ok","version":"1.16.1","type":"navidrome","serverVersion":"0.64.2 (abc)","openSubsonic":true$body}}""",
    )

    private inline fun <reified E : SubsonicError> assertThrows(block: () -> Unit): E {
        try {
            block()
        } catch (e: SubsonicError) {
            if (e is E) return e
            fail("Expected ${E::class.simpleName}, got ${e::class.simpleName}: ${e.message}")
        }
        fail("Expected ${E::class.simpleName}, nothing thrown")
        throw AssertionError()
    }

    // ---- Auth -----------------------------------------------------------------

    @Test
    fun tokenIsMd5OfPasswordAndSalt() {
        // The example from the Subsonic API documentation.
        assertEquals("26719a1196d2a940705a59634eb18eab", SubsonicAuth.token("sesame", "c19b2d"))
    }

    @Test
    fun fromPasswordMakesAFreshSaltAndMatchingToken() {
        val a = SubsonicCredentials.fromPassword(" https://x.example ", "bob ", "pw")
        val b = SubsonicCredentials.fromPassword("https://x.example", "bob", "pw")
        assertEquals(32, a.salt.length)
        assertTrue(a.salt != b.salt)
        assertEquals(SubsonicAuth.token("pw", a.salt), a.token)
        assertEquals("bob", a.user)
        assertEquals("https://x.example", a.url)
        assertFalse(a.toString().contains(a.token))
        assertFalse(a.toString().contains(a.salt))
    }

    @Test
    fun pingSendsTokenAuthAndParsesServer() = runTest {
        server.enqueue(ok())
        val info = client().ping()
        assertEquals("0.64.2 (abc)", info.serverVersion)
        assertEquals("navidrome", info.type)
        assertTrue(info.openSubsonic)

        val url = server.takeRequest().requestUrl!!
        assertEquals("/rest/ping.view", url.encodedPath)
        assertEquals("alice", url.queryParameter("u"))
        assertEquals("26719a1196d2a940705a59634eb18eab", url.queryParameter("t"))
        assertEquals("c19b2d", url.queryParameter("s"))
        assertEquals("1.16.1", url.queryParameter("v"))
        assertEquals("MarqueePlayer", url.queryParameter("c"))
        assertEquals("json", url.queryParameter("f"))
        assertNull(url.queryParameter("p"))
    }

    @Test
    fun keepsAPathPrefix() = runTest {
        server.enqueue(ok())
        client(path = "/music/").ping()
        assertEquals("/music/rest/ping.view", server.takeRequest().requestUrl!!.encodedPath)
    }

    // ---- Read-only mode ---------------------------------------------------------

    @Test
    fun allowlistIsExactlyThePlansList() {
        val plan = setOf(
            "ping", "getOpenSubsonicExtensions", "getMusicFolders", "getArtists", "getArtist", "getAlbum",
            "getSong", "getArtistInfo2", "getAlbumInfo2", "getTopSongs", "getSimilarSongs2", "getAlbumList2",
            "getRandomSongs", "getSongsByGenre", "getStarred2", "getGenres", "search3", "getPlaylists",
            "getPlaylist", "getPlayQueue", "getLyricsBySongId", "getCoverArt", "stream", "download",
            "getScanStatus", "getUser",
        )
        assertEquals(plan, ReadOnlyPolicy.ALLOWED)
    }

    @Test
    fun refusedWritesNeverReachTheServer() = runTest {
        val c = client(readOnly = true)
        assertEquals("star", assertThrows<SubsonicError.RefusedReadOnly> { c.star(id = "1") }.endpoint)
        assertEquals("unstar", assertThrows<SubsonicError.RefusedReadOnly> { c.unstar(albumId = "1") }.endpoint)
        assertEquals("scrobble", assertThrows<SubsonicError.RefusedReadOnly> { c.scrobble("1") }.endpoint)
        assertEquals("updatePlaylist", assertThrows<SubsonicError.RefusedReadOnly> { c.updatePlaylist("p", listOf("1")) }.endpoint)
        assertEquals(0, server.requestCount)
    }

    @Test
    fun readWriteClientSendsPlaylistAdditions() = runTest {
        val c = client(readOnly = false)
        server.enqueue(ok())
        c.updatePlaylist("p1", listOf("a", "b"))
        val u = server.takeRequest().requestUrl!!
        assertEquals("p1", u.queryParameter("playlistId"))
        assertEquals(listOf("a", "b"), u.queryParameterValues("songIdToAdd"))
    }

    @Test
    fun genresStarredAndSongsByGenreParse() = runTest {
        val c = client()
        server.enqueue(ok(""","genres":{"genre":[{"value":"Rock","songCount":12,"albumCount":3},{"value":"Jazz","songCount":1,"albumCount":0}]}"""))
        val g = c.getGenres()
        assertEquals(listOf("Rock", "Jazz"), g.map { it.value })
        assertEquals(3, g[0].albumCount)
        server.enqueue(ok(""","starred2":{"artist":[{"id":"ar1","name":"X"}],"song":[{"id":"s1","title":"T"}]}"""))
        val s = c.getStarred2()
        assertEquals(1, s.artist.size)
        assertEquals(0, s.album.size)
        assertEquals("s1", s.song.single().id)
        server.enqueue(ok(""","songsByGenre":{"song":[{"id":"s2"}]}"""))
        assertEquals("s2", c.getSongsByGenre("Rock", 10).single().id)
        server.takeRequest(); server.takeRequest()
        assertEquals("Rock", server.takeRequest().requestUrl!!.queryParameter("genre"))
    }

    @Test
    fun interceptorRefusesWritesBuiltElsewhere() {
        val c = client(readOnly = true)
        val writes = listOf(
            "scrobble", "star", "unstar", "setRating", "createPlaylist", "updatePlaylist", "deletePlaylist",
            "savePlayQueue", "playbackReport", "createShare", "startScan", "changePassword", "",
        )
        for (endpoint in writes) {
            val url = server.url("/rest/$endpoint.view?id=1")
            try {
                c.httpClient.newCall(Request.Builder().url(url).build()).execute().close()
                fail("$endpoint was sent")
            } catch (e: ReadOnlyRefusal) {
                // expected
            }
        }
        // A path with no rest/ segment is refused too.
        try {
            c.httpClient.newCall(Request.Builder().url(server.url("/other")).build()).execute().close()
            fail("non-rest request was sent")
        } catch (e: ReadOnlyRefusal) {
            // expected
        }
        assertEquals(0, server.requestCount)
    }

    @Test
    fun readOnlyStillAllowsReads() = runTest {
        server.enqueue(ok(""","albumList2":{"album":[{"id":"a1","name":"First","artist":"X","songCount":9,"duration":2400,"coverArt":"al-a1"},{"id":"a2","name":"Second"}]}"""))
        val albums = client(readOnly = true).getAlbumList2(AlbumListType.Newest, size = 5)
        assertEquals(listOf("a1", "a2"), albums.map { it.id })
        assertEquals(9, albums[0].songCount)
        val url = server.takeRequest().requestUrl!!
        assertEquals("/rest/getAlbumList2.view", url.encodedPath)
        assertEquals("newest", url.queryParameter("type"))
        assertEquals("5", url.queryParameter("size"))
        assertNull(url.queryParameter("genre"))
    }

    @Test
    fun writesAreSentWhenNotReadOnly() = runTest {
        server.enqueue(ok())
        client(readOnly = false).star(id = "s1")
        val url = server.takeRequest().requestUrl!!
        assertEquals("/rest/star.view", url.encodedPath)
        assertEquals("s1", url.queryParameter("id"))
    }

    @Test
    fun urlBuilders() {
        val c = client(readOnly = true)
        val cover = c.coverArtUrl("al-1", size = 300).toHttpUrl()
        assertEquals("/rest/getCoverArt.view", cover.encodedPath)
        assertEquals("300", cover.queryParameter("size"))
        assertEquals("al-1", cover.queryParameter("id"))
        val stream = c.streamUrl("t1", format = "opus", maxBitRate = 128).toHttpUrl()
        assertEquals("/rest/stream.view", stream.encodedPath)
        assertEquals("opus", stream.queryParameter("format"))
        assertEquals("128", stream.queryParameter("maxBitRate"))
        assertEquals("json", stream.queryParameter("f"))
        val raw = c.streamUrl("t1").toHttpUrl()
        assertNull(raw.queryParameter("format"))
    }

    // ---- Parsing ----------------------------------------------------------------

    /** The server still sends star ratings (`userRating`); the app no longer reads them, and the answer parses as before. */
    @Test
    fun ratingsFromTheServerAreIgnored() = runTest {
        val c = client()
        server.enqueue(ok(""","album":{"id":"a1","name":"LP","userRating":4,"starred":"2026-01-01T00:00:00Z","song":[{"id":"s1","title":"One","userRating":5}]}"""))
        val album = c.getAlbum("a1")
        assertEquals("LP", album.name)
        assertEquals("s1", album.song.single().id)
        assertEquals("2026-01-01T00:00:00Z", album.starred)
    }

    @Test
    fun parsesScanStatusUserAlbumArtistsSearchPlaylists() = runTest {
        val c = client()
        server.enqueue(ok(""","scanStatus":{"scanning":false,"count":12345,"folderCount":300,"lastScan":"2026-11-01T10:00:00Z"}"""))
        assertEquals(12345L, c.getScanStatus().count)

        server.enqueue(ok(""","user":{"username":"alice","scrobblingEnabled":true,"adminRole":false,"streamRole":true,"folder":[1]}"""))
        val user = c.getUser()
        assertEquals("alice", user.username)
        assertTrue(user.streamRole)
        server.takeRequest() // getScanStatus
        assertEquals("alice", server.takeRequest().requestUrl!!.queryParameter("username"))

        server.enqueue(ok(""","album":{"id":"a1","name":"LP","year":1999,"songCount":2,"song":[{"id":"s1","title":"One","track":1,"discNumber":1,"duration":200,"replayGain":{"trackGain":-6.5,"albumGain":-7.1}},{"id":"s2","title":"Two","track":2}]}"""))
        val album = c.getAlbum("a1")
        assertEquals(listOf("One", "Two"), album.song.map { it.title })
        assertEquals(-6.5, album.song[0].replayGain!!.trackGain!!, 0.0)

        server.enqueue(ok(""","artists":{"ignoredArticles":"The","index":[{"name":"A","artist":[{"id":"r1","name":"Abba","albumCount":3}]},{"name":"B","artist":[{"id":"r2","name":"Bjork"}]}]}"""))
        assertEquals(listOf("Abba", "Bjork"), c.getArtists().all.map { it.name })

        server.enqueue(ok(""","artist":{"id":"r1","name":"Abba","album":[{"id":"a1","name":"LP"}]}"""))
        assertEquals("LP", c.getArtist("r1").album.single().name)

        server.enqueue(ok(""","searchResult3":{"song":[{"id":"s1","title":"One"}]}"""))
        val found = c.search3("one")
        assertEquals(1, found.song.size)
        assertTrue(found.album.isEmpty())

        server.enqueue(ok(""","playlists":{"playlist":[{"id":"p1","name":"Mix","songCount":4}]}"""))
        assertEquals("Mix", c.getPlaylists().single().name)

        server.enqueue(ok(""","playlist":{"id":"p1","name":"Mix","entry":[{"id":"s1","title":"One"}]}"""))
        assertEquals("s1", c.getPlaylist("p1").entry.single().id)

        server.enqueue(ok(""","openSubsonicExtensions":[{"name":"songLyrics","versions":[1]},{"name":"transcodeOffset","versions":[1]}]"""))
        assertEquals(listOf("songLyrics", "transcodeOffset"), c.getOpenSubsonicExtensions().map { it.name })
    }

    @Test
    fun parsesArtistInfoAndTopSongs() = runTest {
        val c = client()
        server.enqueue(ok(""","artistInfo2":{"biography":"Bio","largeImageUrl":"https://example.org/share/img/x?size=1200","similarArtist":[{"id":"r9","name":"Y"}]}"""))
        val info = c.getArtistInfo2("r1")
        assertEquals("https://example.org/share/img/x?size=1200", info.largeImageUrl)
        assertEquals("r1", server.takeRequest().requestUrl!!.queryParameter("id"))

        server.enqueue(ok(""","topSongs":{"song":[{"id":"s1","title":"Hit"},{"id":"s2","title":"Other"}]}"""))
        assertEquals(listOf("s1", "s2"), c.getTopSongs("Abba", count = 2).map { it.id })
        val top = server.takeRequest().requestUrl!!
        assertEquals("Abba", top.queryParameter("artist"))
        assertEquals("2", top.queryParameter("count"))

        server.enqueue(ok(""","topSongs":{}"""))
        assertTrue(c.getTopSongs("Nobody").isEmpty())
        server.enqueue(ok(""","artistInfo2":{}"""))
        assertEquals(null, c.getArtistInfo2("r2").largeImageUrl)
    }

    // ---- Errors -----------------------------------------------------------------

    @Test
    fun wrongPasswordIsAuthFailed() = runTest {
        server.enqueue(MockResponse().setBody("""{"subsonic-response":{"status":"failed","version":"1.16.1","error":{"code":40,"message":"Wrong username or password"}}}"""))
        assertEquals(40, assertThrows<SubsonicError.AuthFailed> { client().ping() }.code)
    }

    @Test
    fun otherSubsonicErrorIsServer() = runTest {
        server.enqueue(MockResponse().setBody("""{"subsonic-response":{"status":"failed","version":"1.16.1","error":{"code":70,"message":"Album not found"}}}"""))
        val e = assertThrows<SubsonicError.Server> { client().getAlbum("x") }
        assertEquals(70, e.code)
        assertNull(e.httpStatus)
    }

    @Test
    fun aStatusThatIsNotAStringIsStillASubsonicError() = runTest {
        server.enqueue(MockResponse().setBody("""{"subsonic-response":{"status":{"x":1}}}"""))
        assertThrows<SubsonicError.Server> { client().ping() }
        server.enqueue(MockResponse().setBody("""{"subsonic-response":{"status":["ok"]}}"""))
        assertThrows<SubsonicError.Server> { client().ping() }
    }

    @Test
    fun htmlPageIsNotSubsonic() = runTest {
        server.enqueue(MockResponse().setBody("<html><body>Welcome</body></html>"))
        assertThrows<SubsonicError.NotSubsonic> { client().ping() }
        server.enqueue(MockResponse().setResponseCode(404).setBody("Not found"))
        assertThrows<SubsonicError.NotSubsonic> { client().ping() }
        server.enqueue(MockResponse().setBody("""{"hello":"world"}"""))
        assertThrows<SubsonicError.NotSubsonic> { client().ping() }
    }

    @Test
    fun http5xxIsServer() = runTest {
        server.enqueue(MockResponse().setResponseCode(503).setBody("busy"))
        assertEquals(503, assertThrows<SubsonicError.Server> { client().ping() }.httpStatus)
    }

    @Test
    fun unreachableIsNetwork() = runTest {
        val c = client()
        server.shutdown()
        assertThrows<SubsonicError.Network> { c.ping() }
    }

    @Test
    fun unknownHostIsNetworkUnknownHost() = runTest {
        val c = SubsonicClient(SubsonicCredentials("https://no-such-host.invalid", "u", "s", "t"), readOnly = true)
        assertEquals(SubsonicError.Network.Kind.UnknownHost, assertThrows<SubsonicError.Network> { c.ping() }.kind)
    }

    @Test
    fun normalizesWhatTheUserTyped() {
        assertEquals("https://music.example.com/", SubsonicClient.normalizeUrl("music.example.com")!!.toString())
        assertEquals("https://music.example.com/", SubsonicClient.normalizeUrl(" https://music.example.com/ ")!!.toString())
        assertEquals("https://music.example.com/nd", SubsonicClient.normalizeUrl("https://music.example.com/nd/rest/")!!.toString())
        assertEquals("http://10.0.2.2:4533/", SubsonicClient.normalizeUrl("http://10.0.2.2:4533")!!.toString())
        assertNull(SubsonicClient.normalizeUrl(""))
        assertNull(SubsonicClient.normalizeUrl("ftp://x"))
        assertNull(SubsonicClient.normalizeUrl("https://"))
    }

    @Test
    fun invalidUrlIsRefusedUpFront() {
        try {
            SubsonicClient(SubsonicCredentials("not a url at all ::", "u", "s", "t"), readOnly = true)
            fail("accepted")
        } catch (e: SubsonicError.InvalidUrl) {
            // expected
        }
    }

    @Test
    fun urlWithUsernameOrPasswordIsRefused() {
        val withCreds = listOf(
            "https://me:secret@music.example.com",
            "https://me@music.example.com",
            "https://:secret@music.example.com",
            "me:secret@music.example.com",
            "https://me:secret@music.example.com/nd/rest/",
        )
        for (raw in withCreds) {
            assertNull("expected null for $raw", SubsonicClient.normalizeUrl(raw))
            assertTrue("expected hasUrlCredentials for $raw", SubsonicClient.hasUrlCredentials(raw))
        }
        // An '@' in the path is not userinfo and stays allowed.
        assertEquals("https://music.example.com/@user", SubsonicClient.normalizeUrl("https://music.example.com/@user")!!.toString())
        assertFalse(SubsonicClient.hasUrlCredentials("https://music.example.com/@user"))
        assertFalse(SubsonicClient.hasUrlCredentials("not a url ::"))
        try {
            SubsonicClient(SubsonicCredentials("https://me:secret@music.example.com", "u", "s", "t"), readOnly = true)
            fail("accepted URL with embedded credentials")
        } catch (e: SubsonicError.InvalidUrl) {
            assertTrue(e.hasCredentials)
        }
    }

    // ---- Redirects --------------------------------------------------------------

    @Test
    fun defaultHttpClientDisablesAutomaticRedirects() {
        val http = SubsonicClient.defaultHttpClient()
        assertFalse(http.followRedirects)
        assertFalse(http.followSslRedirects)
    }

    @Test
    fun redirectToAnotherHostOrPortIsRefusedWithoutSendingCredentials() = runTest {
        val other = MockWebServer()
        other.start()
        try {
            val target = other.url("/rest/ping.view?u=alice&t=26719a1196d2a940705a59634eb18eab&s=c19b2d")
            server.enqueue(MockResponse().setResponseCode(302).setHeader("Location", target.toString()))
            val c = client()
            val err = assertThrows<SubsonicError.Network> { c.ping() }
            assertTrue("expected ProtocolException, got ${err.cause}", err.cause is ProtocolException)
            assertEquals("signed query must never reach another server", 0, other.requestCount)

            // Same policy for direct client.httpClient callers (streams, downloads, cover art).
            val streamTarget = other.url("/rest/stream.view?id=t1&u=alice&t=26719a1196d2a940705a59634eb18eab&s=c19b2d")
            server.enqueue(MockResponse().setResponseCode(302).setHeader("Location", streamTarget.toString()))
            try {
                c.httpClient.newCall(Request.Builder().url(c.streamUrl("t1")).build()).execute().close()
                fail("redirect to another server was followed")
            } catch (e: ProtocolException) {
                // expected
            }
            assertEquals(0, other.requestCount)
        } finally {
            other.shutdown()
        }
    }

    @Test
    fun redirectFromHttpsToHttpIsRefusedWithoutSendingCredentials() = runTest {
        val seen = mutableListOf<String>()
        val terminal = Interceptor { chain ->
            val req = chain.request()
            seen += req.url.toString()
            val location = req.url.newBuilder().scheme("http").port(80).build().toString()
            Response.Builder()
                .request(req)
                .protocol(Protocol.HTTP_1_1)
                .code(302)
                .message("Found")
                .header("Location", location)
                .body("".toResponseBody("text/plain".toMediaType()))
                .build()
        }
        val c = SubsonicClient(
            SubsonicCredentials("https://music.example.com", "alice", "c19b2d", "26719a1196d2a940705a59634eb18eab"),
            readOnly = true,
            httpClient = SubsonicClient.defaultHttpClient().newBuilder().addInterceptor(terminal).build(),
        )
        val err = assertThrows<SubsonicError.Network> { c.ping() }
        assertTrue("expected ProtocolException, got ${err.cause}", err.cause is ProtocolException)
        assertEquals(1, seen.size)
        assertTrue(seen.single().startsWith("https://"))
        assertTrue("signed query must never be sent over http", seen.none { it.startsWith("http://") })
    }

    @Test
    fun redirectOnTheSameOriginIsFollowedUpToFiveHops() = runTest {
        val c = client()
        // Five hops on the same scheme + host + port succeed.
        for (hop in 1..5) {
            server.enqueue(MockResponse().setResponseCode(302).setHeader("Location", "/hop$hop/rest/ping.view?u=alice&t=26719a1196d2a940705a59634eb18eab&s=c19b2d"))
        }
        server.enqueue(ok())
        val info = c.ping()
        assertEquals("ok", info.status)
        assertEquals(6, server.requestCount)
        repeat(6) { server.takeRequest() }

        // A sixth redirect hop is refused.
        for (hop in 1..6) {
            server.enqueue(MockResponse().setResponseCode(302).setHeader("Location", "/loop$hop/rest/ping.view?u=alice&t=26719a1196d2a940705a59634eb18eab&s=c19b2d"))
        }
        val err = assertThrows<SubsonicError.Network> { c.ping() }
        assertTrue("expected ProtocolException, got ${err.cause}", err.cause is ProtocolException)
        assertEquals(12, server.requestCount)
    }

    @Test
    fun sameOriginRedirectCannotBypassReadOnlyGuard() = runTest {
        server.enqueue(MockResponse().setResponseCode(302).setHeader("Location", "/rest/scrobble.view?id=s1"))
        val err = assertThrows<SubsonicError.RefusedReadOnly> { client(readOnly = true).ping() }
        assertEquals("scrobble", err.endpoint)
        assertEquals(1, server.requestCount)
    }
}
