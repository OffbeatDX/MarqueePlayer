package com.offbeat.marquee.core.diag

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.time.ZoneOffset

class DiagnosticsTest {
    @get:Rule val tmp = TemporaryFolder()

    // A made-up account, shaped like a real one.
    private val url = "https://music.example.com"
    private val user = "Dana.Friend"
    private val salt = "9f1c2e7a5b3d4c6e8f0a1b2c3d4e5f60"
    private val token = "26719a1196d2a940705a59634eb18eab"
    private val scrubber = Scrubber.of(url, user, salt, token)

    private fun assertClean(out: String) {
        for (s in listOf(salt, token, user, "music.example.com", "example.com")) {
            assertFalse("'$s' left in: $out", out.contains(s, ignoreCase = true))
        }
    }

    // ---- The server address (release Settings) --------------------------------------------

    @Test
    fun serverAddressIsTheHostWithoutSchemeOrQuery() {
        assertEquals("music.example.com", ServerAddress.display("https://music.example.com"))
        assertEquals("music.example.com", ServerAddress.display("https://music.example.com/"))
        assertEquals("music.example.com", ServerAddress.display("HTTPS://Music.Example.COM"))
        assertEquals("music.example.com", ServerAddress.display("music.example.com"))
        assertEquals("music.example.com", ServerAddress.display("https://music.example.com/rest/ping?u=dana&t=abc&s=def"))
        assertEquals("music.example.com", ServerAddress.display("https://dana:secret@music.example.com/#top"))
    }

    @Test
    fun serverAddressKeepsANonDefaultPortAndAPath() {
        assertEquals("music.example.com:4533", ServerAddress.display("https://music.example.com:4533"))
        assertEquals("music.example.com", ServerAddress.display("https://music.example.com:443"))
        assertEquals("music.example.com:80", ServerAddress.display("https://music.example.com:80"))
        assertEquals("example.com/navidrome", ServerAddress.display("https://example.com/navidrome/"))
        assertEquals("example.com/apps/navidrome", ServerAddress.display("https://example.com/apps/navidrome/rest"))
        assertEquals("192.168.1.20:4533", ServerAddress.display("http://192.168.1.20:4533"))
        assertEquals("[2001:db8::1]:4533", ServerAddress.display("https://[2001:db8::1]:4533"))
    }

    @Test
    fun serverAddressShowsAnInternationalNameInItsOwnLetters() {
        assertEquals("musik.bücher.example", ServerAddress.display("https://musik.xn--bcher-kva.example"))
        assertEquals("musik.bücher.example", ServerAddress.display("https://musik.bücher.example"))
    }

    @Test
    fun theAccountLineIsTheAddressAndTheServerVersion() {
        assertEquals("music.example.com · Navidrome 0.64.2", accountSummary("https://music.example.com", "Navidrome 0.64.2"))
        assertEquals("example.com:8443/navidrome", accountSummary("https://example.com:8443/navidrome", null))
        assertEquals("Navidrome 0.64.2", accountSummary("not a url", "Navidrome 0.64.2"))
        assertEquals("Signed in", accountSummary("", " "))
        // Never the token or salt, even if a URL carrying them was saved.
        val line = accountSummary("https://music.example.com/?u=$user&t=$token&s=$salt", "Navidrome 0.64.2")
        assertFalse(line.contains(token) || line.contains(salt) || line.contains(user))
    }

    @Test
    fun serverAddressIsNullForWhatIsNoUrl() {
        assertNull(ServerAddress.display(""))
        assertNull(ServerAddress.display("   "))
        assertNull(ServerAddress.display("ftp://music.example.com"))
        assertNull(ServerAddress.display("https://"))
    }

    // ---- The scrubber ---------------------------------------------------------------------

    @Test
    fun streamAndCoverUrlsBecomeUrlMarks() {
        val line = "load error: $url/rest/stream?u=$user&t=$token&s=$salt&v=1.16.1&c=MarqueePlayer&f=json&id=al-42&format=opus&maxBitRate=128 failed"
        val out = scrubber.scrub(line)
        assertEquals("load error: <url> failed", out)
        val cover = scrubber.scrub("coil: ($url:443/rest/getCoverArt?id=al-7&size=300&u=$user&t=$token&s=$salt) 404")
        assertEquals("coil: (<url>) 404", cover)
    }

    @Test
    fun anyUrlIsRemovedEvenWithoutAnAccount() {
        val out = Scrubber.NONE.scrub("GET https://other.example.org/rest/getAlbum?id=1&u=bob&t=x then \"http://10.0.0.2:4533/x\"")
        assertEquals("GET <url> then \"<url>\"", out)
    }

    @Test
    fun authParametersLoseTheirValuesOutsideAUrl() {
        val out = Scrubber.NONE.scrub("request /rest/ping.view?u=bob&t=abcdef&s=123456&p=enc:6869&v=1.16.1 refused")
        assertEquals("request /rest/ping.view?u=<hidden>&t=<hidden>&s=<hidden>&p=<hidden>&v=1.16.1 refused", out)
        // Percent-encoded separators, as in a URL inside another URL's query.
        val enc = Scrubber.NONE.scrub("next=%2Frest%2Fstream%3Fid%3D9%26u%3Dbob%26t%3Dabcdef%26s%3D1234")
        assertEquals("next=%2Frest%2Fstream%3Fid%3D9%26u%3D<hidden>%26t%3D<hidden>%26s%3D<hidden>", enc)
        // Ordinary words that merely contain u= or s= are left alone.
        assertEquals("status=ok bytes=12", Scrubber.NONE.scrub("status=ok bytes=12"))
    }

    @Test
    fun theHostOnItsOwnIsReplaced() {
        val out = scrubber.scrub("UnknownHostException: Unable to resolve host \"music.example.com\": No address associated with hostname")
        assertEquals("UnknownHostException: Unable to resolve host \"<server>\": No address associated with hostname", out)
        assertEquals("connect to <server>:443 timed out", scrubber.scrub("connect to MUSIC.Example.com:443 timed out"))
        // The domain above it names the owner as well.
        assertEquals("certificate for <server> expired", scrubber.scrub("certificate for example.com expired"))
    }

    @Test
    fun theHostsAddressAfterItIsReplacedToo() {
        assertEquals(
            "ConnectException: Failed to connect to <server>",
            scrubber.scrub("ConnectException: Failed to connect to music.example.com/203.0.113.5:443"),
        )
        assertEquals("to <server> (x)", scrubber.scrub("to music.example.com/2001:db8::1:4533 (x)"))
        assertEquals("to <server> (x)", scrubber.scrub("to music.example.com/[2001:db8::1]:4533 (x)"))
        assertEquals("a/b 10.0.0.1:80", scrubber.scrub("a/b 10.0.0.1:80"))
        assertEquals("<server>/feed", scrubber.scrub("music.example.com/feed"))
    }

    @Test
    fun theUserNameIsReplacedInsideASentence() {
        assertEquals("Signed in as <user> on <server>", scrubber.scrub("Signed in as Dana.Friend on music.example.com"))
        assertEquals("user '<user>' not authorised", scrubber.scrub("user 'dana.friend' not authorised"))
    }

    @Test
    fun encodedFormsAreReplacedToo() {
        val s = Scrubber.of("https://musik.bücher.example", "dana friend@home", salt, token)
        assertEquals("host <server>", s.scrub("host musik.xn--bcher-kva.example"))
        assertEquals("host <server>", s.scrub("host musik.bücher.example"))
        assertEquals("q=<server>", s.scrub("q=musik.b%C3%BCcher.example"))
        assertEquals("name=<user>", s.scrub("name=dana+friend%40home"))
        assertEquals("name=<user>", s.scrub("name=dana%20friend%40home"))
        assertEquals("name=<user>", s.scrub("name=DANA%20FRIEND%40HOME"))
        // The percent-encoded host of the test account, in a percent-encoded URL.
        assertEquals("<url>", scrubber.scrub("https%3A%2F%2Fmusic.example.com%2Frest%2Fstream%3Fid%3D1"))
    }

    @Test
    fun tokenAndSaltAreReplacedWhereverTheyAre() {
        assertEquals("t: <token> s: <salt>", scrubber.scrub("t: $token s: $salt"))
        assertEquals("T=<token>", scrubber.scrub("T=${token.uppercase()}"))
        assertEquals("salted:<salt>!", scrubber.scrub("salted:$salt!"))
    }

    @Test
    fun aShortUserNameIsOnlyReplacedAsAWord() {
        val s = Scrubber.of(user = "al")
        assertEquals("<user> already played", s.scrub("al already played"))
        assertEquals("signed in as <user>.", s.scrub("signed in as AL."))
    }

    @Test
    fun scrubbersAddUp() {
        val both = Scrubber.of(user = "first") + Scrubber.of(url = "https://second.example.net")
        assertEquals("<user> on <server>", both.scrub("first on second.example.net"))
        assertSame(scrubber, scrubber + Scrubber.NONE)
        assertTrue(Scrubber.NONE.isEmpty)
    }

    @Test
    fun aRealisticLogExcerptComesOutClean() {
        val log = """
            2026-11-17 10:23:45.123 I MarqueePlayback: now: so-81 (auto)
            2026-11-17 10:23:46.001 W MarqueePlayback: playback error: ERROR_CODE_IO_NETWORK_CONNECTION_FAILED (HttpDataSourceException)
                androidx.media3.datasource.HttpDataSource${'$'}HttpDataSourceException: java.net.UnknownHostException: Unable to resolve host "music.example.com"
                Caused by: java.io.IOException: GET $url/rest/stream?u=$user&t=$token&s=$salt&v=1.16.1&id=so-81
            2026-11-17 10:23:47.500 I MarqueeSettings: signed in as $user (salt $salt)
        """.trimIndent()
        val out = scrubber.scrub(log)
        assertClean(out)
        assertTrue(out.contains("now: so-81 (auto)"))
        assertTrue(out.contains("Unable to resolve host \"<server>\""))
        assertTrue(out.contains("GET <url>"))
    }

    // ---- The ring log ---------------------------------------------------------------------

    @Test
    fun theLogRotatesAndNeverOutgrowsItsCap() {
        val log = RingLog(tmp.newFolder("logs"), maxBytes = 2_000)
        repeat(200) { log.append("line %03d xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\n".format(it)) }
        assertTrue(log.sizeBytes() <= 2_000)
        val text = log.read()
        assertTrue(text.endsWith("line 199 xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\n"))
        assertFalse(text.contains("line 000"))
        // At least the latest half is kept, oldest first.
        assertTrue(text.lines().size > 20)
        val numbers = text.lines().filter { it.isNotEmpty() }.map { it.substring(5, 8).toInt() }
        assertEquals(numbers.sorted(), numbers)
    }

    @Test
    fun anOversizedEntryKeepsItsEnd() {
        val log = RingLog(tmp.newFolder("big"), maxBytes = 100)
        log.append("a".repeat(500) + "END\n")
        assertTrue(log.sizeBytes() <= 50)
        assertTrue(log.read().endsWith("END\n"))
    }

    @Test
    fun clearingTheLogRemovesBothFiles() {
        val log = RingLog(tmp.newFolder("clear"), maxBytes = 200)
        repeat(20) { log.append("0123456789\n") }
        assertTrue(log.previous.isFile)
        log.clear()
        assertFalse(log.current.exists())
        assertFalse(log.previous.exists())
        assertEquals("", log.read())
    }

    @Test
    fun aLogLineHasTimeLevelTagAndIndentedTrace() {
        val t = java.time.LocalDateTime.of(2026, 11, 17, 10, 23, 45, 123_000_000).toInstant(ZoneOffset.UTC).toEpochMilli()
        val line = LogLine.format(t, 'W', "MarqueeNet", "offline\nsecond", "java.io.IOException: x\n\tat a.b(C.kt:1)\n", ZoneOffset.UTC)
        assertEquals(
            "2026-11-17 10:23:45.123 W MarqueeNet: offline\n    second\n    java.io.IOException: x\n    \tat a.b(C.kt:1)\n",
            line,
        )
        val long = LogLine.format(t, 'I', "T", "x".repeat(LogLine.MAX_MESSAGE + 10), zone = ZoneOffset.UTC)
        assertTrue(long.contains("(2010 chars)"))
    }

    // ---- Crashes and the report -----------------------------------------------------------

    @Test
    fun theCrashHandlerRecordsThenHandsOver() {
        val seen = mutableListOf<String>()
        val previous = Thread.UncaughtExceptionHandler { _, e -> seen += "previous: ${e.message}" }
        val h = CrashHandler(previous) { t, e -> seen += "recorded on ${t.name}: ${e.message}" }
        val thread = Thread {}.apply { name = "worker" }
        h.uncaughtException(thread, IllegalStateException("boom"))
        assertEquals(listOf("recorded on worker: boom", "previous: boom"), seen)
    }

    @Test
    fun aFailingRecorderStillHandsOver() {
        var handedOver = false
        val h = CrashHandler({ _, _ -> handedOver = true }) { _, _ -> throw OutOfMemoryError("while recording") }
        h.uncaughtException(Thread.currentThread(), RuntimeException("x"))
        assertTrue(handedOver)
    }

    @Test
    fun aCrashOnARealThreadReachesTheInstalledHandler() {
        val before = Thread.getDefaultUncaughtExceptionHandler()
        val recorded = mutableListOf<String>()
        try {
            CrashHandler.install { _, e -> synchronized(recorded) { recorded += e.message!! } }
            val t = Thread { throw IllegalArgumentException("thrown on a thread") }
            t.start()
            t.join()
            assertEquals(listOf("thrown on a thread"), recorded)
        } finally {
            Thread.setDefaultUncaughtExceptionHandler(before)
        }
    }

    @Test
    fun theCrashDescriptionIsScrubbed() {
        val e = RuntimeException("GET $url/rest/getAlbum?id=1&u=$user&t=$token&s=$salt failed for $user")
        val text = CrashHandler.describe(0L, Thread.currentThread(), e, scrubber)
        assertClean(text)
        assertTrue(text.contains("java.lang.RuntimeException: GET <url> failed for <user>"))
        assertTrue(text.contains("at com.offbeat.marquee.core.diag.DiagnosticsTest"))
    }

    @Test
    fun theReportIsScrubbedAgainAsAWhole() {
        val report = DiagnosticReport("Marquee Player diagnostics")
            .section("App", listOf("Version" to "0.1.0", "Build" to "release"))
            .section("Settings", listOf("Streaming on Wi-Fi" to "Original", "Server" to "Navidrome 0.64.2"))
            .text("Last crash", null)
            .text("Log", "I X: user $user on music.example.com t=$token")
            .render(scrubber)
        assertClean(report)
        assertTrue(report.startsWith("Marquee Player diagnostics\n\n== App ==\nVersion  0.1.0\nBuild    release\n"))
        assertTrue(report.contains("== Last crash ==\n(none)\n"))
        assertTrue(report.contains("I X: user <user> on <server> t=<token>"))
    }
}
