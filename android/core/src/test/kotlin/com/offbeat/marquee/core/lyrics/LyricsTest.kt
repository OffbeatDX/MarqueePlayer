package com.offbeat.marquee.core.lyrics

import com.offbeat.marquee.core.subsonic.SubsonicClient
import com.offbeat.marquee.core.subsonic.SubsonicCredentials
import com.offbeat.marquee.core.subsonic.SubsonicError
import kotlinx.coroutines.test.runTest
import kotlinx.serialization.json.Json
import okhttp3.OkHttpClient
import okhttp3.mockwebserver.Dispatcher
import okhttp3.mockwebserver.MockResponse
import okhttp3.mockwebserver.MockWebServer
import okhttp3.mockwebserver.RecordedRequest
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Before
import org.junit.Test
import java.io.File
import java.nio.file.Files
import java.util.concurrent.TimeUnit

class LrcTest {
    private fun times(l: Lyrics) = l.lines.map { it.startMs to it.text }

    @Test
    fun parsesLrcWithItsVariants() {
        val l = Lrc.parse(
            "[ar:Someone]\n[ti:Song]\n[00:12.34]First line\n[00:05]Intro\n[01:02:50]Colon cs\n" +
                "[00:20.00][00:40.5]Chorus\n\n[1:00:00.00]An hour in\n[00:07.123]Millis\n",
            LyricsSource.Lrclib,
        )
        assertTrue(l.synced)
        assertEquals(
            listOf(
                5_000L to "Intro", 7_123L to "Millis", 12_340L to "First line", 20_000L to "Chorus",
                40_500L to "Chorus", 62_500L to "Colon cs", 3_600_000L to "An hour in",
            ),
            times(l),
        )
    }

    @Test
    fun keepsTheOffsetAndStripsEnhancedStamps() {
        val l = Lrc.parse("[offset:+500]\n[00:10.00]v1: <00:10.00>Hello <00:10.50>there <00:11.00>world\n[00:12.00]a <b> c\n", LyricsSource.Lrclib)
        assertEquals(listOf(10_000L to "Hello there world", 12_000L to "a <b> c"), times(l))
        assertEquals(500L, l.offsetMs)
        // A positive offset shows lines sooner.
        assertEquals(9_500L, l.lineStart(0))
        assertEquals(0, l.currentLine(9_500))
        assertEquals(-1, l.currentLine(9_499))
        val neg = Lrc.parse("[offset:-250]\n[00:01.00]x", LyricsSource.Lrclib)
        assertEquals(1_250L, neg.lineStart(0))
    }

    @Test
    fun blankStampedLinesStayAndBlankUnstampedOnesGo() {
        val l = Lrc.parse("﻿[00:01.00]One\n\n[00:05.00]\n   \n[00:09.00]Two", LyricsSource.Lrclib)
        assertEquals(listOf(1_000L to "One", 5_000L to "", 9_000L to "Two"), times(l))
    }

    @Test
    fun plainTextIsUnsynced() {
        val l = Lrc.parse("\n\nVerse one\nstill one\n\nVerse two\n\n", LyricsSource.Server)
        assertFalse(l.synced)
        assertEquals(listOf(null to "Verse one", null to "still one", null to "", null to "Verse two"), times(l))
    }

    @Test
    fun notStampsStayText() {
        assertNull(Lrc.stamp("ab:cd"))
        assertNull(Lrc.stamp("12"))
        assertNull(Lrc.stamp("1:2:3:4"))
        assertEquals(83_000L, Lrc.stamp("01:23"))
        assertEquals(83_400L, Lrc.stamp("01:23.4"))
        val l = Lrc.parse("[Chorus]\nla la", LyricsSource.Server)
        assertFalse(l.synced)
        assertEquals("[Chorus]", l.lines[0].text)
    }

    @Test
    fun sectionHeadersWithAColonAreTextNotTags() {
        val l = Lrc.parse("[ar:Someone]\n[Verse 1: Someone]\nfirst\n[Chorus: Other]\n[Intro: Drake] Yeah, yeah\n[by:x] kept", LyricsSource.Server)
        assertFalse(l.synced)
        assertEquals(
            listOf("[Verse 1: Someone]", "first", "[Chorus: Other]", "[Intro: Drake] Yeah, yeah", "kept"),
            l.lines.map { it.text },
        )
        val s = Lrc.parse("[length:3:00]\n[00:01.00][Verse: X] words", LyricsSource.Lrclib)
        assertEquals(listOf(1_000L to "[Verse: X] words"), times(s))
    }

    @Test
    fun theCurrentLineAndTheNextChange() {
        val l = Lrc.parse("[00:01.00]a\n[00:03.00]b\n[00:03.00]b2\n[00:08.00]c", LyricsSource.Lrclib)
        assertEquals(-1, l.currentLine(0))
        assertEquals(1_000L, l.nextChangeAt(0))
        assertEquals(0, l.currentLine(2_999))
        assertEquals(2, l.currentLine(3_000)) // the last of two lines on one stamp
        assertEquals(8_000L, l.nextChangeAt(3_000))
        assertEquals(3, l.currentLine(100_000))
        assertNull(l.nextChangeAt(100_000))
    }

    @Test
    fun formatRoundTrips() {
        val l = Lrc.parse("[offset:120]\n[00:01.05]a\n[01:03.999]b\n[00:30.00]", LyricsSource.Lrclib)
        assertEquals(l, Lrc.parse(Lrc.format(l), LyricsSource.Lrclib))
        val p = Lrc.parse("x\n\ny", LyricsSource.Server)
        assertEquals(p, Lrc.parse(Lrc.format(p), LyricsSource.Server))
    }
}

class ServerLyricsTest {
    private val json = Json { ignoreUnknownKeys = true }

    private fun entries(body: String) = json.decodeFromString(
        kotlinx.serialization.builtins.ListSerializer(StructuredLyrics.serializer()),
        body,
    )

    @Test
    fun syncedFirstThenTheDeviceLanguageThenTheFirst() {
        val list = entries(
            """[
              {"lang":"eng","synced":false,"line":[{"value":"plain english"}]},
              {"lang":"deu","synced":true,"offset":100,"line":[{"start":2000,"value":"zwei"},{"start":1000,"value":"eins"}]},
              {"lang":"eng","synced":true,"line":[{"start":1500,"value":"one"}]}
            ]""",
        )
        val en = ServerLyrics.choose(list, listOf("en", "eng"))!!
        assertTrue(en.synced)
        assertEquals("one", en.lines[0].text)
        assertEquals("eng", en.lang)
        val de = ServerLyrics.choose(list, listOf("de"))!!
        assertEquals(listOf("eins", "zwei"), de.lines.map { it.text }) // sorted by start
        assertEquals(100L, de.offsetMs)
        // No language matches: the first synced one.
        assertEquals("zwei", ServerLyrics.choose(list, listOf("ja"))!!.lines[1].text)
        // Only plain ones: the language, else the first.
        val plain = entries("""[{"lang":"xxx","synced":false,"line":[{"value":"a"}]},{"lang":"fra","synced":false,"line":[{"value":"b"}]}]""")
        assertEquals("b", ServerLyrics.choose(plain, listOf("fr"))!!.lines[0].text)
        assertEquals("a", ServerLyrics.choose(plain, listOf("en"))!!.lines[0].text)
        assertNull(ServerLyrics.choose(plain, listOf("en"))!!.lang) // "xxx" is no language
    }

    @Test
    fun emptyEntriesAreSkippedAndPlainLrcIsRead() {
        assertNull(ServerLyrics.choose(entries("""[{"synced":false,"line":[{"value":"  "}]},{"synced":true,"line":[]}]"""), listOf("en")))
        val lrc = ServerLyrics.choose(entries("""[{"synced":false,"line":[{"value":"[00:01.00]one"},{"value":"[00:02.50]two"}]}]"""), listOf("en"))!!
        assertTrue(lrc.synced)
        assertEquals(2_500L, lrc.lines[1].startMs)
        assertEquals(LyricsSource.Server, lrc.source)
    }

    @Test
    fun theClientReadsGetLyricsBySongId() = runTest {
        val server = MockWebServer()
        server.start()
        try {
            server.enqueue(
                MockResponse().setBody(
                    """{"subsonic-response":{"status":"ok","version":"1.16.1","openSubsonic":true,"lyricsList":{"structuredLyrics":[
                    {"displayArtist":"A","displayTitle":"T","lang":"eng","offset":-100,"synced":true,"line":[{"start":0,"value":"Hi"},{"start":2000,"value":"There"}]}]}}}""",
                ),
            )
            server.enqueue(MockResponse().setBody("""{"subsonic-response":{"status":"ok","version":"1.16.1","lyricsList":{}}}"""))
            val c = SubsonicClient(
                SubsonicCredentials(server.url("/").toString(), "alice", "salt", "token"),
                readOnly = true,
                httpClient = OkHttpClient.Builder().readTimeout(5, TimeUnit.SECONDS).build(),
            )
            val list = c.getLyricsBySongId("s1")
            assertEquals(1, list.size)
            assertEquals("T", list[0].displayTitle)
            assertEquals(-100L, list[0].offset)
            val req = server.takeRequest()
            assertTrue(req.path!!.startsWith("/rest/getLyricsBySongId.view?"))
            assertEquals("s1", req.requestUrl!!.queryParameter("id"))
            assertTrue(c.getLyricsBySongId("s2").isEmpty())
        } finally {
            server.shutdown()
        }
    }
}

class LrclibMatchTest {
    private val t = LyricsTrack("s", "Beyoncé", "Halo", "I Am... Sasha Fierce", 261.4)

    private fun rec(artist: String, title: String, duration: Double, id: Long = 1, album: String? = null, synced: String? = null, plain: String? = "words") =
        LrclibRecord(id = id, trackName = title, artistName = artist, albumName = album, duration = duration, plainLyrics = plain, syncedLyrics = synced)

    @Test
    fun matchingFoldsAndChecksTheLength() {
        assertTrue(LrclibMatch.matches(t, rec("BEYONCE", "halo", 262.0)))
        assertTrue(LrclibMatch.matches(t, rec("beyoncé", "  Halo ", 259.5)))
        assertFalse("a different edit", LrclibMatch.matches(t, rec("Beyoncé", "Halo", 264.0)))
        assertFalse(LrclibMatch.matches(t, rec("Beyoncé", "Halo (Remix)", 261.0)))
        assertFalse(LrclibMatch.matches(t, rec("Someone Else", "Halo", 261.0)))
        val r = rec("Beyonce", "Halo", 261.0)
        assertTrue(LrclibMatch.acceptable(t, r))
        assertFalse("no words", LrclibMatch.acceptable(t, r.copy(plainLyrics = null)))
        assertTrue("instrumental", LrclibMatch.acceptable(t, r.copy(plainLyrics = null, instrumental = true)))
    }

    @Test
    fun foldingMatchesTheDesktop() {
        assertEquals("dvorak", Fold.fold("Dvořák"))
        assertEquals("strasse", Fold.fold("Straße"))
        assertEquals("abc 123", Fold.fold("ＡＢＣ　１２３"))
        assertEquals(Fold.fold("カタカナ"), Fold.fold("かたかな"))
        assertEquals(Fold.fold("ｶﾞｷﾞ"), Fold.fold("ガギ"))
        assertFalse("voicing marks stay", Fold.fold("が") == Fold.fold("か"))
        assertEquals("sigur ros", Fold.fold("  Sigur   Rós "))
        assertEquals("bjork", Fold.fold("BJÖRK"))
    }

    @Test
    fun theBestResultIsTheSameAlbumThenSyncedThenTheClosestLength() {
        val a = rec("Beyonce", "Halo", 262.5, id = 1)
        val b = rec("Beyonce", "Halo", 261.0, id = 2)
        val c = rec("Beyonce", "Halo", 263.0, id = 3, synced = "[00:01.00]x")
        val d = rec("Beyonce", "Halo", 263.3, id = 4, album = "I am... Sasha Fierce")
        val wrong = rec("Beyonce", "Halo", 300.0, id = 5, album = "I Am... Sasha Fierce", synced = "[00:01.00]x")
        assertEquals(4L, LrclibMatch.best(t, listOf(a, b, c, d, wrong))!!.id)
        assertEquals(3L, LrclibMatch.best(t, listOf(a, b, c))!!.id)
        assertEquals(2L, LrclibMatch.best(t, listOf(a, b))!!.id)
        assertNull(LrclibMatch.best(t, listOf(wrong)))
    }

    @Test
    fun recordsParseAsTheApiSendsThem() {
        val json = Json { ignoreUnknownKeys = true; coerceInputValues = true }
        val r = json.decodeFromString(
            LrclibRecord.serializer(),
            """{"id":3396226,"name":"I Want to Live","trackName":"I Want to Live","artistName":"Borislav Slavov","albumName":"Baldur's Gate 3","duration":233,"instrumental":false,"plainLyrics":"I feel your breath upon my neck\nA soft caress","syncedLyrics":"[00:17.12] I feel your breath upon my neck\n[00:20.85] A soft caress"}""",
        )
        val a = r.answer() as LyricsAnswer.Found
        assertTrue(a.lyrics.synced)
        assertEquals(20_850L, a.lyrics.lines[1].startMs)
        assertEquals("I feel your breath upon my neck", a.lyrics.lines[0].text)
        assertEquals(LyricsSource.Lrclib, a.lyrics.source)
        val inst = json.decodeFromString(LrclibRecord.serializer(), """{"id":5,"trackName":"x","artistName":"y","albumName":null,"duration":100.5,"instrumental":true,"plainLyrics":null,"syncedLyrics":null}""")
        assertEquals(LyricsAnswer.Instrumental, inst.answer())
        // Plain lyrics that look stamped are still plain.
        val odd = LrclibRecord(id = 6, plainLyrics = "[00:01.00]x", syncedLyrics = "  ")
        val oa = odd.answer() as LyricsAnswer.Found
        assertFalse(oa.lyrics.synced)
        assertNull(oa.lyrics.lines[0].startMs)
    }

    @Test
    fun lookable() {
        assertTrue(t.lookable)
        assertFalse(t.copy(durationSec = 0.0).lookable)
        assertFalse(t.copy(artist = " ").lookable)
    }
}

/** LRCLIB against a local mock: never lrclib.net. */
class LrclibClientTest {
    private lateinit var server: MockWebServer
    private val t = LyricsTrack("s1", "Beyoncé", "Halo", "I Am... Sasha Fierce", 261.4)
    private val http = OkHttpClient.Builder().readTimeout(5, TimeUnit.SECONDS).build()

    @Before
    fun setUp() {
        server = MockWebServer()
        server.start()
    }

    @After
    fun tearDown() {
        runCatching { server.shutdown() }
    }

    private fun client() = LrclibClient(http, server.url("/").toString(), LrclibClient.userAgent("0.1.0"))

    @Test
    fun theExactLookupComesFirstWithAllFourFields() = runTest {
        server.enqueue(MockResponse().setBody("""{"id":9,"trackName":"Halo","artistName":"Beyoncé","duration":261,"syncedLyrics":"[00:01.00]Remember"}"""))
        val a = client().lookup(t) as LyricsAnswer.Found
        assertEquals("Remember", a.lyrics.lines[0].text)
        val req = server.takeRequest()
        val url = req.requestUrl!!
        assertEquals("/api/get", url.encodedPath)
        assertEquals("Beyoncé", url.queryParameter("artist_name"))
        assertEquals("Halo", url.queryParameter("track_name"))
        assertEquals("I Am... Sasha Fierce", url.queryParameter("album_name"))
        assertEquals("261", url.queryParameter("duration"))
        assertEquals("MarqueePlayer/0.1.0 (Android)", req.getHeader("User-Agent"))
        assertEquals(1, server.requestCount)
    }

    @Test
    fun aMissFallsBackToTheStrictSearch() = runTest {
        server.enqueue(MockResponse().setResponseCode(404).setBody("""{"code":404,"name":"TrackNotFound"}"""))
        server.enqueue(
            MockResponse().setBody(
                """[{"id":1,"trackName":"Halo (Live)","artistName":"Beyoncé","duration":261,"plainLyrics":"wrong"},
                   {"id":2,"trackName":"HALO","artistName":"Beyonce","duration":262.9,"plainLyrics":"right"},
                   {"id":3,"trackName":"Halo","artistName":"Beyonce","duration":250,"plainLyrics":"other edit"}]""",
            ),
        )
        val a = client().lookup(t) as LyricsAnswer.Found
        assertEquals("right", a.lyrics.lines[0].text)
        assertEquals("/api/get", server.takeRequest().requestUrl!!.encodedPath)
        val search = server.takeRequest().requestUrl!!
        assertEquals("/api/search", search.encodedPath)
        assertEquals("Halo", search.queryParameter("track_name"))
        assertEquals("Beyoncé", search.queryParameter("artist_name"))
    }

    @Test
    fun nothingAcceptableIsNotFound() = runTest {
        server.enqueue(MockResponse().setResponseCode(404))
        server.enqueue(MockResponse().setBody("""[{"id":1,"trackName":"Halo","artistName":"Someone","duration":261,"plainLyrics":"x"}]"""))
        assertEquals(LyricsAnswer.NotFound, client().lookup(t))
    }

    @Test
    fun testBuildsReachOnlyTheLoopback() = runTest {
        val guarded = http.newBuilder().addInterceptor(LoopbackOnly()).build()
        // The mock is on the loopback: allowed.
        server.enqueue(MockResponse().setResponseCode(404))
        server.enqueue(MockResponse().setBody("[]"))
        assertEquals(LyricsAnswer.NotFound, LrclibClient(guarded, server.url("/").toString(), "x").lookup(t))
        // The real service: refused before anything is sent (no DNS, no connection).
        try {
            LrclibClient(guarded, LrclibClient.PUBLIC_URL, "x").lookup(t)
            fail()
        } catch (e: LrclibError.Network) {
            assertTrue(e.cause!!.message!!.startsWith("Refused by read-only mode"))
        }
    }

    @Test
    fun busyAndServerErrorsAreTyped() = runTest {
        server.enqueue(MockResponse().setResponseCode(429).setHeader("Retry-After", "30"))
        try {
            client().lookup(t)
            fail()
        } catch (e: LrclibError.Busy) {
            assertEquals(30L, e.retryAfterSec)
        }
        server.enqueue(MockResponse().setResponseCode(500))
        try {
            client().lookup(t)
            fail()
        } catch (e: LrclibError.Server) {
        }
        server.enqueue(MockResponse().setBody("<html>not json</html>"))
        try {
            client().lookup(t)
            fail()
        } catch (e: LrclibError.Server) {
        }
    }
}

class LyricsResolverTest {
    private lateinit var dir: File
    private lateinit var server: MockWebServer
    private var now = 1_000_000_000_000L
    private var serverLyrics: List<StructuredLyrics> = emptyList()
    private var serverCalls = 0
    private var serverFails: Throwable? = null
    private var online = true
    private val t = LyricsTrack("song-1", "Beyoncé", "Halo", "I Am... Sasha Fierce", 261.4)

    /** LRCLIB answers: what /api/get and /api/search return. */
    private var exact: MockResponse = MockResponse().setResponseCode(404)
    private var search: MockResponse = MockResponse().setBody("[]")

    @Before
    fun setUp() {
        dir = Files.createTempDirectory("lyrics-cache").toFile()
        server = MockWebServer()
        server.dispatcher = object : Dispatcher() {
            override fun dispatch(request: RecordedRequest): MockResponse = when (request.requestUrl!!.encodedPath) {
                "/api/get" -> exact
                "/api/search" -> search
                else -> MockResponse().setResponseCode(404)
            }
        }
        server.start()
    }

    @After
    fun tearDown() {
        runCatching { server.shutdown() }
        dir.deleteRecursively()
    }

    private val cache by lazy { LyricsCache(dir, clock = { now }) }
    private val lrclib by lazy { LrclibClient(OkHttpClient(), server.url("/").toString(), "MarqueePlayer/test (Android)") }

    private fun resolver() = LyricsResolver(
        cache,
        server = { serverCalls++; serverFails?.let { throw it }; serverLyrics },
        lrclib = { if (online) lrclib else null },
        deviceLanguages = { listOf("en") },
        isNetworkError = { it is SubsonicError.Network },
    )

    private fun synced(text: String) = StructuredLyrics(lang = "eng", synced = true, line = listOf(StructuredLine(1000, text)))
    private fun plain(text: String) = StructuredLyrics(lang = "eng", synced = false, line = listOf(StructuredLine(null, text)))

    private fun text(a: LyricsAnswer) = (a as LyricsAnswer.Found).lyrics.lines.first().text
    private fun source(a: LyricsAnswer) = (a as LyricsAnswer.Found).lyrics.source

    @Test
    fun theSourceOrder() {
        val ss = Lyrics(true, listOf(LyricLine(1, "ss")), LyricsSource.Server)
        val sp = Lyrics(false, listOf(LyricLine(null, "sp")), LyricsSource.Server)
        val ls = Lyrics(true, listOf(LyricLine(1, "ls")), LyricsSource.Lrclib)
        val lp = Lyrics(false, listOf(LyricLine(null, "lp")), LyricsSource.Lrclib)
        assertEquals("ss", text(SourceOrder.pick(ss, LyricsAnswer.Found(ls))))
        assertEquals("ls", text(SourceOrder.pick(sp, LyricsAnswer.Found(ls))))
        assertEquals("sp", text(SourceOrder.pick(sp, LyricsAnswer.Found(lp))))
        assertEquals("lp", text(SourceOrder.pick(null, LyricsAnswer.Found(lp))))
        assertEquals("sp", text(SourceOrder.pick(sp, LyricsAnswer.Instrumental)))
        assertEquals(LyricsAnswer.Instrumental, SourceOrder.pick(null, LyricsAnswer.Instrumental))
        assertEquals(LyricsAnswer.NotFound, SourceOrder.pick(null, LyricsAnswer.NotFound))
        assertEquals(LyricsAnswer.NotFound, SourceOrder.pick(null, null))
    }

    @Test
    fun serverSyncedNeverAsksLrclib() = runTest {
        serverLyrics = listOf(synced("from server"))
        val a = resolver().resolve(t)
        assertEquals("from server", text(a))
        assertEquals(LyricsSource.Server, source(a))
        assertEquals(0, server.requestCount)
        // Cached: nothing is asked again.
        val r = resolver()
        assertNotNull(r.fromCache(t))
        r.resolve(t)
        assertEquals(1, serverCalls)
    }

    @Test
    fun lrclibSyncedBeatsServerPlain() = runTest {
        serverLyrics = listOf(plain("server words"))
        exact = MockResponse().setBody("""{"id":1,"trackName":"Halo","artistName":"Beyoncé","duration":261,"syncedLyrics":"[00:01.00]online words"}""")
        val a = resolver().resolve(t)
        assertEquals("online words", text(a))
        assertEquals(LyricsSource.Lrclib, source(a))
        // Online off: LRCLIB's answer is kept but not shown (as on the desktop); on again, it is back.
        online = false
        assertEquals("server words", text(resolver().fromCache(t)!!))
        online = true
        assertEquals("online words", text(resolver().fromCache(t)!!))
        assertEquals(1, server.requestCount)
    }

    @Test
    fun serverPlainBeatsLrclibPlain() = runTest {
        serverLyrics = listOf(plain("server words"))
        exact = MockResponse().setBody("""{"id":1,"trackName":"Halo","artistName":"Beyoncé","duration":261,"plainLyrics":"online words"}""")
        assertEquals("server words", text(resolver().resolve(t)))
    }

    @Test
    fun notFoundIsCachedAndAskedAgainAfterAboutTwoWeeks() = runTest {
        val r = resolver()
        assertEquals(LyricsAnswer.NotFound, r.resolve(t))
        assertEquals(1, serverCalls)
        assertEquals(2, server.requestCount) // get, then search
        // Cached "not found": no requests at all.
        assertEquals(LyricsAnswer.NotFound, r.fromCache(t))
        assertEquals(LyricsAnswer.NotFound, r.resolve(t))
        assertEquals(1, serverCalls)
        assertEquals(2, server.requestCount)
        // 13 days later: still cached.
        now += 13L * 24 * 3600 * 1000
        assertNotNull(r.fromCache(t))
        // 15 days: both sources are asked again, once.
        now += 2L * 24 * 3600 * 1000
        assertNull(r.fromCache(t))
        exact = MockResponse().setBody("""{"id":1,"trackName":"Halo","artistName":"Beyoncé","duration":261,"syncedLyrics":"[00:01.00]added later"}""")
        assertEquals("added later", text(r.resolve(t)))
        assertEquals(2, serverCalls)
        assertEquals(3, server.requestCount)
        // Found lyrics are kept, however long it has been.
        now += 400L * 24 * 3600 * 1000
        assertEquals("added later", text(r.fromCache(t)!!))
    }

    @Test
    fun offlineLookupIsTriedOnceLrclibIsTurnedOn() = runTest {
        online = false
        assertEquals(LyricsAnswer.NotFound, resolver().resolve(t))
        assertEquals(0, server.requestCount)
        online = true
        val r = resolver()
        assertNull("LRCLIB was never asked", r.fromCache(t))
        exact = MockResponse().setBody("""{"id":1,"trackName":"Halo","artistName":"Beyoncé","duration":261,"plainLyrics":"now"}""")
        assertEquals("now", text(r.resolve(t)))
        assertEquals("the server's answer is reused", 1, serverCalls)
    }

    @Test
    fun refreshAsksTheServerAgainAndSearchOnlineAsksLrclibAgain() = runTest {
        val r = resolver()
        r.resolve(t)
        serverLyrics = listOf(synced("tagged since"))
        assertEquals(LyricsAnswer.NotFound, r.resolve(t)) // the cache still says none
        assertEquals("tagged since", text(r.resolve(t, LyricsResolver.Mode.Refresh)))
        assertEquals(2, serverCalls)

        val t2 = t.copy(songId = "song-2")
        serverLyrics = emptyList()
        r.resolve(t2)
        val before = server.requestCount
        exact = MockResponse().setBody("""{"id":1,"trackName":"Halo","artistName":"Beyoncé","duration":261,"plainLyrics":"found now"}""")
        assertEquals("found now", text(r.resolve(t2, LyricsResolver.Mode.SearchOnline)))
        assertEquals(before + 1, server.requestCount)
        assertEquals(3, serverCalls) // SearchOnline does not ask the server again
    }

    @Test
    fun errorsAreNotCachedAndAFoundAnswerWinsOverThem() = runTest {
        serverFails = SubsonicError.Network(SubsonicError.Network.Kind.Timeout, null)
        try {
            resolver().resolve(t.copy(artist = "")) // not lookable, so only the server is asked
            fail()
        } catch (e: LyricsLookupError) {
            assertFalse(e.busy)
        }
        assertNull(cache.read("song-1"))
        // A server without the endpoint counts as "none".
        serverFails = SubsonicError.Server(0, 404, "not implemented")
        exact = MockResponse().setBody("""{"id":1,"trackName":"Halo","artistName":"Beyoncé","duration":261,"plainLyrics":"online"}""")
        assertEquals("online", text(resolver().resolve(t)))
        // The server has plain words and LRCLIB is busy: the words are shown, LRCLIB asked later.
        val t3 = t.copy(songId = "song-3")
        serverFails = null
        serverLyrics = listOf(plain("server plain"))
        exact = MockResponse().setResponseCode(429).setHeader("Retry-After", "120")
        val r = resolver()
        assertEquals("server plain", text(r.resolve(t3)))
        assertNull("LRCLIB still to ask", r.fromCache(t3))
        // While busy LRCLIB is not asked at all.
        val count = server.requestCount
        assertEquals("server plain", text(r.resolve(t3)))
        assertEquals(count, server.requestCount)
        // Busy with nothing to show is an error that says so.
        val t4 = t.copy(songId = "song-4")
        serverLyrics = emptyList()
        try {
            r.resolve(t4)
            fail()
        } catch (e: LyricsLookupError) {
            assertTrue(e.busy)
        }
    }
}

class LyricsCacheTest {
    @Test
    fun leastRecentlyUsedOutPastTheCapAndKeysAreSongIds() {
        val dir = Files.createTempDirectory("lyrics-lru").toFile()
        try {
            var now = 1_000_000L
            val words = Lyrics(false, List(20) { LyricLine(null, "line $it of some words") }, LyricsSource.Server)
            // One entry's size, to set a cap that holds three.
            LyricsCache(dir, clock = { now }).apply { write("probe", LyricsEntry(server = words, serverAt = now)) }
            val one = dir.listFiles()!!.single().length()
            dir.listFiles()!!.forEach { it.delete() }
            val cap = one * 3 + one / 2
            val cache = LyricsCache(dir, maxBytes = cap, clock = { now })
            for (i in 1..3) {
                cache.write("id$i", LyricsEntry(server = words, serverAt = now))
                now += 10_000
            }
            assertNotNull(cache.read("id1")) // id1 is now the most recent
            now += 10_000
            cache.write("id4", LyricsEntry(server = words, serverAt = now))
            assertTrue(cache.sizeBytes() <= cap)
            assertNotNull(cache.read("id1"))
            assertNull("the least recently used went", cache.read("id2"))
            assertNotNull(cache.read("id4"))
            assertEquals(setOf("id1.json", "id3.json", "id4.json"), dir.list()!!.toSet())
            assertEquals("a_b~" + Integer.toHexString("a/b".hashCode()), LyricsCache.safeName("a/b"))
        } finally {
            dir.deleteRecursively()
        }
    }
}
