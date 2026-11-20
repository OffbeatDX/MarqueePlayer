package com.offbeat.marquee.core.offline

import com.offbeat.marquee.core.playback.CacheKeys
import com.offbeat.marquee.core.playback.NetworkKind
import com.offbeat.marquee.core.playback.PlaybackRecovery
import com.offbeat.marquee.core.playback.QualitySettings
import com.offbeat.marquee.core.playback.Scrobbler
import com.offbeat.marquee.core.playback.SourcePicker
import com.offbeat.marquee.core.playback.StallRule
import com.offbeat.marquee.core.playback.StreamQuality
import com.offbeat.marquee.core.subsonic.Song
import com.offbeat.marquee.core.subsonic.SubsonicClient
import com.offbeat.marquee.core.subsonic.SubsonicCredentials
import com.offbeat.marquee.core.subsonic.SubsonicError
import com.offbeat.marquee.core.sync.Backoff
import com.offbeat.marquee.core.sync.FlushResult
import com.offbeat.marquee.core.sync.Outbox
import com.offbeat.marquee.core.sync.OutboxSender
import com.offbeat.marquee.core.sync.TextFile
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import okhttp3.mockwebserver.MockResponse
import okhttp3.mockwebserver.MockWebServer
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** Downloads (pins and what they share), what plays offline, and the outbox of scrobbles and favourites. */
class OfflineTest {
    private fun songs(vararg ids: String) = ids.map { Song(it, title = it) }
    private fun album(id: String, vararg ids: String) = Pin(PinKind.Album, id, songs = songs(*ids))
    private fun playlist(id: String, vararg ids: String) = Pin(PinKind.Playlist, id, songs = songs(*ids))

    // ---- Pins: reference counting --------------------------------------------------------

    @Test
    fun pinningFetchesOnlyWhatIsNew() {
        val a = PinSet().with(album("al", "1", "2", "3"))
        assertEquals(setOf("1", "2", "3"), a.fetch)
        assertTrue(a.release.isEmpty())
        val b = a.pins.with(playlist("pl", "3", "4"))
        assertEquals(setOf("4"), b.fetch) // 3 is stored once
        assertEquals(listOf("1", "2", "3", "4"), b.pins.wanted().toList())
        assertEquals(2, b.pins.refCount("3"))
        assertEquals(1, b.pins.refCount("1"))
    }

    @Test
    fun sharedSongsAreDeletedOnlyWhenNothingPinsThem() {
        val set = PinSet().with(album("al", "1", "2", "3")).pins.with(playlist("pl", "3", "4")).pins
        val noAlbum = set.without("album:al")
        assertEquals(setOf("1", "2"), noAlbum.release) // 3 stays: the playlist still pins it
        assertEquals(1, noAlbum.pins.refCount("3"))
        val nothing = noAlbum.pins.without("playlist:pl")
        assertEquals(setOf("3", "4"), nothing.release)
        assertTrue(nothing.pins.pins.isEmpty())
        // Pinning the same album twice keeps one pin.
        assertEquals(1, PinSet().with(album("al", "1")).pins.with(album("al", "1")).pins.pins.size)
        assertEquals(setOf("1", "2", "3", "4"), set.cleared().release)
    }

    @Test
    fun aSongPinnedAloneAndOnAnAlbumSurvivesEitherRemoval() {
        val set = PinSet().with(album("al", "1", "2")).pins.with(Pin(PinKind.Song, "2", songs = songs("2"))).pins
        assertEquals(setOf("1"), set.without("album:al").release)
        assertTrue(set.without("song:2").release.isEmpty())
    }

    // ---- Pinned playlists follow the server ---------------------------------------------------

    @Test
    fun aPinnedPlaylistFollowsTheServer() {
        val set = PinSet().with(album("al", "1", "2")).pins.with(playlist("pl", "2", "3", "4")).pins
        val change = set.followPlaylist("pl", songs("4", "5", "1"), changed = "t2", nowMs = 10)
        assertEquals(setOf("5"), change.fetch) // 1 is already stored for the album
        assertEquals(setOf("3"), change.release) // 2 stays for the album, 3 goes
        val pin = change.pins["playlist:pl"]!!
        assertEquals(listOf("4", "5", "1"), pin.songIds)
        assertEquals("t2", pin.changed)
        assertEquals(10L, pin.syncedAtMs)
        // Not pinned: nothing changes.
        assertEquals(set, set.followPlaylist("other", songs("9"), null, 0).pins)
    }

    @Test
    fun listDiffAndReconcile() {
        val d = ListDiff.of(listOf("1", "2", "3"), listOf("3", "4", "1", "4"))
        assertEquals(listOf("4"), d.added)
        assertEquals(listOf("2"), d.removed)
        assertTrue(ListDiff.of(listOf("1"), listOf("1")).isEmpty)
        val r = Reconcile.of(wanted = listOf("1", "2", "3"), stored = setOf("2", "3", "9"), complete = setOf("2"))
        assertEquals(listOf("1", "3"), r.fetch) // 3 is partly stored: it resumes
        assertEquals(setOf("9"), r.delete)
    }

    @Test
    fun pinsSurviveEncoding() {
        val set = PinSet().with(album("al", "1")).pins.with(playlist("pl", "2")).pins
        assertEquals(set, PinSet.decode(set.encode()))
        assertEquals(PinSet(), PinSet.decode("not json"))
        assertEquals(PinSet(), PinSet.decode(null))
    }

    // ---- What plays offline -----------------------------------------------------------------

    @Test
    fun offlinePlayabilityRules() {
        assertTrue(Playable.isPlayable(online = true, downloaded = false, fullyCached = false))
        assertTrue(Playable.isPlayable(online = false, downloaded = true, fullyCached = false))
        assertTrue(Playable.isPlayable(online = false, downloaded = false, fullyCached = true))
        assertFalse(Playable.isPlayable(online = false, downloaded = false, fullyCached = false))
    }

    @Test
    fun playOfflinePlaysWhatIsAvailable() {
        val list = listOf("a", "b", "c", "d", "e")
        val stored = setOf("b", "d")
        assertEquals(listOf("b", "d") to 0, Playable.queueFrom(list, 0) { it in stored })
        assertEquals(listOf("b", "d") to 1, Playable.queueFrom(list, 2) { it in stored }) // from c: d is next
        assertEquals(listOf("b", "d") to 1, Playable.queueFrom(list, 4) { it in stored }) // nothing after: the last one
        assertNull(Playable.queueFrom(list, 0) { false })
        assertNull(Playable.queueFrom(emptyList<String>(), 0) { true })
    }

    @Test
    fun thePlayerSkipsToTheNextPlayableEntry() {
        val order = listOf(3, 0, 4, 1, 2) // shuffle order
        val ok = setOf(3, 2)
        assertEquals(2, Playable.nextIn(order, 0, wrap = false) { it in ok })
        assertNull(Playable.nextIn(order, 2, wrap = false) { it in ok })
        assertEquals(3, Playable.nextIn(order, 2, wrap = true) { it in ok })
        assertNull(Playable.nextIn(order, 3, wrap = true) { it == 99 })
    }

    @Test
    fun aDownloadedCopyWinsInWhateverQuality() {
        val downloaded = setOf(CacheKeys.song("s", StreamQuality.opus(192)))
        val cached = setOf(CacheKeys.song("s", StreamQuality.ORIGINAL))
        for (n in NetworkKind.entries) {
            assertEquals(StreamQuality.opus(192), SourcePicker.pick("s", n, QualitySettings(), isDownloaded = { it in downloaded }) { it in cached })
        }
        assertEquals(StreamQuality.ORIGINAL, SourcePicker.pick("s", NetworkKind.Metered, QualitySettings(), isDownloaded = { false }) { it in cached })
    }

    // ---- The outbox: scrobbles and favourites waiting for the network ----------------------------

    private class MemFile : TextFile {
        var text: String? = null
        var writes = 0
        override fun read() = text
        override fun write(text: String) {
            this.text = text
            writes++
        }
    }

    private class FakeSender : OutboxSender {
        val calls = mutableListOf<String>()
        var fail: SubsonicError? = null
        override suspend fun scrobble(ids: List<String>, timesMs: List<Long>) {
            fail?.let { throw it }
            calls += "scrobble " + ids.zip(timesMs).joinToString(",") { (i, t) -> "$i@$t" }
        }
        override suspend fun star(kind: String, id: String, starred: Boolean) {
            fail?.let { throw it }
            calls += "star $kind $id $starred"
        }
    }

    private val offline = SubsonicError.Network(SubsonicError.Network.Kind.UnknownHost, null)

    @Test
    fun scrobblesGoOutInPlayOrderInOneBatchWithFavouritesFirst() = runTest {
        var now = 0L
        val outbox = Outbox(MemFile(), { now })
        outbox.addScrobble("b", 2_000)
        outbox.addScrobble("a", 1_000)
        outbox.addScrobble("c", 3_000)
        outbox.setStar("song", "x", starred = true, atMs = 5)
        val sender = FakeSender()
        assertEquals(FlushResult.Done, outbox.flush(sender))
        assertEquals(listOf("star song x true", "scrobble a@1000,b@2000,c@3000"), sender.calls)
        assertTrue(outbox.isEmpty)
        assertEquals(FlushResult.Empty, outbox.flush(sender))
    }

    @Test
    fun duplicatesAreQueuedOnceAndTheLastFavouriteChangeWins() {
        val outbox = Outbox(MemFile(), { 0 })
        outbox.addScrobble("a", 1_000)
        outbox.addScrobble("a", 1_000)
        outbox.addScrobble("a", 9_000) // a second play of the same song is its own scrobble
        outbox.setStar("song", "x", true, 1)
        outbox.setStar("song", "x", false, 2)
        val d = outbox.snapshot()
        assertEquals(2, d.scrobbles.size)
        assertEquals(1, d.stars.size)
        assertFalse(d.stars[0].starred)
        assertEquals(false, outbox.pendingStar("x")?.starred)
    }

    @Test
    fun theQueueSurvivesARestart() = runTest {
        val file = MemFile()
        Outbox(file, { 0 }).apply {
            addScrobble("a", 1)
            addScrobble("b", 2)
            setStar("album", "al", true, 3)
        }
        val reopened = Outbox(file, { 0 })
        assertEquals(3, reopened.size)
        val sender = FakeSender()
        reopened.flush(sender)
        assertEquals(listOf("star album al true", "scrobble a@1,b@2"), sender.calls)
        assertTrue(Outbox(file, { 0 }).isEmpty) // and what was sent stays sent
    }

    @Test
    fun aNetworkFailureKeepsEverythingAndBacksOffOnAFakeClock() = runTest {
        var now = 0L
        val file = MemFile()
        val outbox = Outbox(file, { now }, Backoff(60_000, 600_000))
        outbox.addScrobble("a", 1)
        val sender = FakeSender().apply { fail = offline }
        assertEquals(FlushResult.Retry(60_000), outbox.flush(sender))
        assertEquals(1, outbox.size)
        assertFalse(outbox.isDue())
        now = 59_999
        assertEquals(FlushResult.NotDue(60_000), outbox.flush(sender))
        now = 60_000
        assertTrue(outbox.isDue())
        assertEquals(FlushResult.Retry(180_000), outbox.flush(sender)) // 2nd failure: 2 min
        now = 180_000
        assertEquals(FlushResult.Retry(420_000), outbox.flush(sender)) // 3rd: 4 min
        now = 420_000
        assertEquals(FlushResult.Retry(900_000), outbox.flush(sender)) // 4th: 8 min
        now = 900_000
        assertEquals(FlushResult.Retry(1_500_000), outbox.flush(sender)) // 5th: capped at 10 min
        // The backoff is kept on disk too.
        assertEquals(1_500_000L, Outbox(file, { now }).snapshot().nextAttemptAtMs)
        // The network is back: forced, it goes, and the backoff resets.
        sender.fail = null
        assertEquals(FlushResult.Done, outbox.flush(sender, force = true))
        assertEquals(listOf("scrobble a@1"), sender.calls)
        assertEquals(0, outbox.snapshot().failures)
    }

    @Test
    fun refusedAndRejectedEntriesAreDroppedNotRetried() = runTest {
        val logs = mutableListOf<String>()
        val outbox = Outbox(MemFile(), { 0 }, log = { logs += it })
        outbox.addScrobble("a", 1)
        outbox.setStar("song", "x", true, 1)
        val sender = FakeSender().apply { fail = SubsonicError.RefusedReadOnly("scrobble") }
        assertEquals(FlushResult.Done, outbox.flush(sender))
        assertTrue(outbox.isEmpty)
        assertEquals(2, logs.count { "refused (read-only build)" in it })
        for (code in listOf(10, 20, 30, 50, 60, 70)) {
            outbox.addScrobble("s$code", code.toLong())
            outbox.setStar("song", "x$code", true, code.toLong())
            sender.fail = SubsonicError.Server(code, null, "refused $code")
            assertEquals(FlushResult.Done, outbox.flush(sender))
            assertTrue("code $code should be dropped", outbox.isEmpty)
            assertEquals(0, outbox.snapshot().failures)
        }
    }

    @Test
    fun transientServerAndCaptivePortalErrorsKeepEntriesAndBackOff() = runTest {
        val transientErrors = listOf(
            SubsonicError.NotSubsonic("unexpected response"),
            SubsonicError.NotSubsonic("HTTP 429"),
            SubsonicError.Server(null, 503, null),
            SubsonicError.Server(null, 408, null),
            SubsonicError.Server(null, 429, null),
            SubsonicError.Server(0, null, "generic error"),
            SubsonicError.AuthFailed(40, "wrong credentials"),
            offline,
        )
        val backoff = Backoff(60_000, 600_000)
        var now = 1_000L
        val outbox = Outbox(MemFile(), { now }, backoff)
        outbox.setStar("song", "x", starred = true, atMs = 10)
        outbox.addScrobble("a", 20)
        val sender = FakeSender()

        for ((index, error) in transientErrors.withIndex()) {
            val expectedFailures = index + 1
            val expectedAt = now + backoff.delayFor(expectedFailures)
            sender.fail = error
            assertEquals("error ${error.message}", FlushResult.Retry(expectedAt), outbox.flush(sender, force = true))
            assertEquals("both items must stay for ${error.message}", 2, outbox.size)
            assertEquals(expectedFailures, outbox.snapshot().failures)
            assertEquals(expectedAt, outbox.snapshot().nextAttemptAtMs)
            now += 1_000L
        }

        // Also verify scrobbles alone (no star ahead of them) back off on the same errors.
        val scrobbleOnly = Outbox(MemFile(), { now }, backoff)
        scrobbleOnly.addScrobble("b", 30)
        sender.fail = SubsonicError.NotSubsonic("captive portal")
        assertEquals(FlushResult.Retry(now + 60_000), scrobbleOnly.flush(sender))
        assertEquals(1, scrobbleOnly.size)
        assertEquals(1, scrobbleOnly.snapshot().failures)

        // Once the server recovers, everything queued goes out and backoff resets.
        sender.fail = null
        assertEquals(FlushResult.Done, outbox.flush(sender, force = true))
        assertTrue(outbox.isEmpty)
        assertEquals(0, outbox.snapshot().failures)
        assertEquals(0L, outbox.snapshot().nextAttemptAtMs)
        assertEquals(listOf("star song x true", "scrobble a@20"), sender.calls)
    }

    @Test
    fun aRefusedScrobbleBatchIsRetriedOneByOneAndDropsOnlyTheBadItems() = runTest {
        val logs = mutableListOf<String>()
        val outbox = Outbox(MemFile(), { 0 }, log = { logs += it })
        outbox.addScrobble("a", 1)
        outbox.addScrobble("bad1", 2)
        outbox.addScrobble("c", 3)
        outbox.addScrobble("bad2", 4)

        val attempts = mutableListOf<List<String>>()
        val sent = mutableListOf<String>()
        val sender = object : OutboxSender {
            override suspend fun scrobble(ids: List<String>, timesMs: List<Long>) {
                attempts += ids
                if (ids.any { it.startsWith("bad") }) throw SubsonicError.Server(70, null, "not found")
                sent += ids.zip(timesMs).map { (i, t) -> "$i@$t" }
            }
            override suspend fun star(kind: String, id: String, starred: Boolean) = Unit
        }

        assertEquals(FlushResult.Done, outbox.flush(sender))
        assertTrue(outbox.isEmpty)
        assertEquals(0, outbox.snapshot().failures)
        assertEquals(listOf(listOf("a", "bad1", "c", "bad2"), listOf("a"), listOf("bad1"), listOf("c"), listOf("bad2")), attempts)
        assertEquals(listOf("a@1", "c@3"), sent)
        assertEquals(2, logs.count { "dropped" in it })
    }

    @Test
    fun aTransientFailureDuringBatchSplitKeepsOnlyTheUnsentItems() = runTest {
        var now = 0L
        val outbox = Outbox(MemFile(), { now }, Backoff(60_000, 600_000))
        outbox.addScrobble("a", 1)
        outbox.addScrobble("bad", 2)
        outbox.addScrobble("c", 3)

        var cFails = true
        val sent = mutableListOf<String>()
        val sender = object : OutboxSender {
            override suspend fun scrobble(ids: List<String>, timesMs: List<Long>) {
                if ("bad" in ids) throw SubsonicError.Server(70, null, "not found")
                if (cFails && "c" in ids) throw SubsonicError.Server(null, 503, null)
                sent += ids.zip(timesMs).map { (i, t) -> "$i@$t" }
            }
            override suspend fun star(kind: String, id: String, starred: Boolean) = Unit
        }

        assertEquals(FlushResult.Retry(60_000), outbox.flush(sender))
        assertEquals(listOf("a@1"), sent)
        assertEquals(listOf("c"), outbox.snapshot().scrobbles.map { it.id })
        assertEquals(1, outbox.snapshot().failures)
        assertEquals(60_000L, outbox.snapshot().nextAttemptAtMs)

        // When the server recovers, only the remaining item is sent.
        cFails = false
        now = 60_000
        assertEquals(FlushResult.Done, outbox.flush(sender))
        assertTrue(outbox.isEmpty)
        assertEquals(listOf("a@1", "c@3"), sent)
        assertEquals(0, outbox.snapshot().failures)
    }

    @Test
    fun theQueueIsCappedAndBatched() = runTest {
        val outbox = Outbox(MemFile(), { 0 }, maxScrobbles = 120, batchSize = 50)
        for (i in 1..130) outbox.addScrobble("s$i", i.toLong())
        assertEquals(120, outbox.size)
        assertEquals("s11", outbox.snapshot().scrobbles.first().id) // the oldest ten went
        val sender = FakeSender()
        outbox.flush(sender)
        assertEquals(3, sender.calls.size) // 50 + 50 + 20
    }

    @Test
    fun readOnlyClientRefusesTheBatchAndSendsNothing() = runTest {
        val server = MockWebServer()
        server.start()
        try {
            val client = SubsonicClient(SubsonicCredentials(server.url("/").toString(), "u", "s", "t"), readOnly = true)
            val sender = object : OutboxSender {
                override suspend fun scrobble(ids: List<String>, timesMs: List<Long>) = client.scrobble(ids, timesMs)
                override suspend fun star(kind: String, id: String, starred: Boolean) = if (starred) client.star(id) else client.unstar(id)
            }
            val logs = mutableListOf<String>()
            val outbox = Outbox(MemFile(), { 0 }, log = { logs += it })
            outbox.addScrobble("a", 1)
            outbox.addScrobble("b", 2)
            outbox.setStar("song", "a", true, 3)
            assertEquals(FlushResult.Done, outbox.flush(sender))
            assertEquals(0, server.requestCount)
            assertTrue(outbox.isEmpty)
            assertEquals(2, logs.count { "dropped" in it })
        } finally {
            server.shutdown()
        }
    }

    @Test
    fun batchScrobbleRepeatsIdAndTime() = runTest {
        val server = MockWebServer()
        server.start()
        try {
            server.enqueue(MockResponse().setBody("""{"subsonic-response":{"status":"ok","version":"1.16.1"}}"""))
            val client = SubsonicClient(SubsonicCredentials(server.url("/").toString(), "u", "s", "t"), readOnly = false)
            client.scrobble(listOf("a", "b"), listOf(10L, 20L))
            val url = server.takeRequest().requestUrl!!
            assertEquals(listOf("a", "b"), url.queryParameterValues("id"))
            assertEquals(listOf("10", "20"), url.queryParameterValues("time"))
            assertEquals("true", url.queryParameter("submission"))
        } finally {
            server.shutdown()
        }
    }

    @Test
    fun theScrobblerQueuesSubmissionsButSendsNowPlaying() = runTest {
        val sent = mutableListOf<Boolean>()
        val queued = mutableListOf<Pair<String, Long>>()
        val s = Scrobbler(this, { testScheduler.currentTime }, { _, _, sub -> sent += sub }, queue = { id, t -> queued += id to t })
        s.onTrackStarted("a", 100_000, playing = true, wallClockMs = 5_000)
        runCurrent()
        advanceTimeBy(75_001); runCurrent()
        assertEquals(listOf(false), sent) // only "now playing" went out directly
        assertEquals(listOf("a" to 5_000L), queued)
    }

    // ---- Playback recovery and the stall watchdog -------------------------------------------------

    @Test
    fun backoffTiming() {
        val b = Backoff(2_000, 30_000)
        assertEquals(listOf(0L, 2_000L, 4_000L, 8_000L, 16_000L, 30_000L, 30_000L), (0..6).map(b::delayFor))
    }

    @Test
    fun aTrackIsRetriedThenSkippedAfterThreeFailures() {
        val r = PlaybackRecovery()
        assertEquals(PlaybackRecovery.Action.Retry(2_000, 1), r.onError("t1", networkError = true, online = true))
        assertEquals(PlaybackRecovery.Action.WaitForNetwork(2), r.onError("t1", networkError = true, online = false))
        assertEquals(PlaybackRecovery.Action.Skip, r.onError("t1", networkError = true, online = true))
        // A new track starts counting afresh; playing resets the count.
        assertEquals(PlaybackRecovery.Action.Retry(2_000, 1), r.onError("t2", true, true))
        assertEquals(PlaybackRecovery.Action.Retry(4_000, 2), r.onError("t2", true, true))
        r.onPlaying("t2")
        assertEquals(PlaybackRecovery.Action.Retry(2_000, 1), r.onError("t2", true, true))
        // Not a network error: retrying won't help.
        assertEquals(PlaybackRecovery.Action.Skip, r.onError("t3", networkError = false, online = true))
    }

    @Test
    fun theStallWatchdogActsOnlyWhenTheTrackIsStored() {
        assertTrue(StallRule.shouldRecover(8_000, playWhenReady = true, trackStored = true, recoveriesThisTrack = 0))
        assertFalse(StallRule.shouldRecover(7_999, true, true, 0))
        assertFalse(StallRule.shouldRecover(60_000, true, trackStored = false, recoveriesThisTrack = 0)) // a slow link: just wait
        assertFalse(StallRule.shouldRecover(60_000, playWhenReady = false, trackStored = true, recoveriesThisTrack = 0))
        assertFalse(StallRule.shouldRecover(60_000, true, true, StallRule.MAX_PER_TRACK))
    }
}
