package com.offbeat.marquee.core.playback

import com.offbeat.marquee.core.subsonic.ReplayGain
import com.offbeat.marquee.core.subsonic.Song
import com.offbeat.marquee.core.subsonic.SubsonicClient
import com.offbeat.marquee.core.subsonic.SubsonicCredentials
import com.offbeat.marquee.core.subsonic.SubsonicError
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.advanceUntilIdle
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import okhttp3.HttpUrl.Companion.toHttpUrl
import okhttp3.mockwebserver.MockResponse
import okhttp3.mockwebserver.MockWebServer
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test
import kotlin.math.abs

@OptIn(ExperimentalCoroutinesApi::class)
class PlaybackRulesTest {

    // ---- Quality rule and stream URLs ---------------------------------------------------

    @Test
    fun unmeteredGetsOriginalMeteredGetsOpus128() {
        val s = QualitySettings()
        assertEquals(StreamQuality.ORIGINAL, s.forNetwork(metered = false))
        assertEquals(StreamQuality.opus(128), s.forNetwork(metered = true))
    }

    @Test
    fun streamUrlsCarryTheFormatOfTheQuality() {
        val c = SubsonicClient(SubsonicCredentials("https://music.example", "u", "s", "t"), readOnly = true)
        val original = c.streamUrl("song1", StreamQuality.ORIGINAL).toHttpUrl()
        assertEquals("/rest/stream.view", original.encodedPath)
        assertEquals("raw", original.queryParameter("format"))
        assertNull(original.queryParameter("maxBitRate"))
        val opus = c.streamUrl("song1", StreamQuality.opus(128)).toHttpUrl()
        assertEquals("opus", opus.queryParameter("format"))
        assertEquals("128", opus.queryParameter("maxBitRate"))
        assertEquals("song1", opus.queryParameter("id"))
    }

    @Test
    fun qualityIdsRoundTripAndUnknownFallsBack() {
        for (q in StreamQuality.CHOICES) assertEquals(q, StreamQuality.fromId(q.id, StreamQuality.ORIGINAL))
        assertEquals(StreamQuality.opus(160), StreamQuality.fromId("opus160", StreamQuality.ORIGINAL))
        assertEquals(StreamQuality.ORIGINAL, StreamQuality.fromId("flac9000", StreamQuality.ORIGINAL))
        assertEquals(StreamQuality.opus(128), StreamQuality.fromId(null, StreamQuality.opus(128)))
    }

    // ---- Cache keys and the copy to play ---------------------------------------------------

    @Test
    fun cacheKeyIsSongIdAndQualityNeverTheUrl() {
        val key = CacheKeys.song("abc123", StreamQuality.opus(128))
        assertEquals("song/abc123/opus128", key)
        assertEquals("abc123", CacheKeys.songId(key))
        assertNull(CacheKeys.songId("https://x/rest/stream.view?id=1&t=tok"))
        assertFalse(key.contains("t="))
        // Two URLs for the same song and quality (different salts and tokens) share one key.
        assertEquals(CacheKeys.song("abc123", StreamQuality.ORIGINAL), CacheKeys.song("abc123", StreamQuality.ORIGINAL))
    }

    @Test
    fun originalFullyCachedIsUsedOnMobileData() {
        val cached = setOf(CacheKeys.song("s", StreamQuality.ORIGINAL))
        assertEquals(StreamQuality.ORIGINAL, SourcePicker.pick("s", NetworkKind.Metered, QualitySettings()) { it in cached })
    }

    @Test
    fun cachedInBothQualitiesPlaysTheOriginal() {
        val cached = setOf(CacheKeys.song("s", StreamQuality.ORIGINAL), CacheKeys.song("s", StreamQuality.opus(128)))
        for (n in NetworkKind.entries) assertEquals(StreamQuality.ORIGINAL, SourcePicker.pick("s", n, QualitySettings()) { it in cached })
    }

    @Test
    fun opusCopyIsNotUsedOnWifiButIsWhenOffline() {
        val cached = setOf(CacheKeys.song("s", StreamQuality.opus(128)))
        assertEquals(StreamQuality.ORIGINAL, SourcePicker.pick("s", NetworkKind.Unmetered, QualitySettings()) { it in cached })
        assertEquals(StreamQuality.opus(128), SourcePicker.pick("s", NetworkKind.Metered, QualitySettings()) { it in cached })
        val other = setOf(CacheKeys.song("s", StreamQuality.opus(64)))
        assertEquals(StreamQuality.opus(64), SourcePicker.pick("s", NetworkKind.None, QualitySettings()) { it in other })
        assertEquals(StreamQuality.opus(128), SourcePicker.pick("s", NetworkKind.None, QualitySettings()) { false })
    }

    // ---- Opus files that need no transcode --------------------------------------------

    private fun opusFile(kbps: Int?) = SourceFile("opus", "audio/ogg", kbps)

    @Test
    fun anOpusFileWithinTwiceTheBitrateStreamsAsItIs() {
        val picked = { file: SourceFile? -> SourcePicker.pick("s", NetworkKind.Metered, QualitySettings(), file) { false } }
        assertEquals(StreamQuality.ORIGINAL, picked(opusFile(180)))
        assertEquals(StreamQuality.ORIGINAL, picked(opusFile(256))) // exactly twice
        assertEquals(StreamQuality.ORIGINAL, picked(opusFile(96))) // below the wanted rate
        assertEquals("song/s/original", CacheKeys.song("s", picked(opusFile(180))))
        // An Opus quality on Wi-Fi follows the same rule.
        val wifiOpus = QualitySettings(unmetered = StreamQuality.opus(192))
        assertEquals(StreamQuality.ORIGINAL, SourcePicker.pick("s", NetworkKind.Unmetered, wifiOpus, opusFile(320)) { false })
    }

    @Test
    fun anOpusFileAboveTwiceTheBitrateIsTranscoded() {
        assertEquals(StreamQuality.opus(128), SourcePicker.pick("s", NetworkKind.Metered, QualitySettings(), opusFile(257)) { false })
        // Opus 64 chosen to save data still transcodes a 180 kbps file.
        val saver = QualitySettings(metered = StreamQuality.opus(64))
        assertEquals(StreamQuality.opus(64), SourcePicker.pick("s", NetworkKind.Metered, saver, opusFile(180)) { false })
        assertEquals(StreamQuality.ORIGINAL, SourcePicker.pick("s", NetworkKind.Metered, saver, opusFile(128)) { false })
    }

    @Test
    fun otherFilesAndUnknownBitratesAreTranscoded() {
        val opus128 = StreamQuality.opus(128)
        for (file in listOf(
            SourceFile("flac", "audio/flac", 900),
            SourceFile("mp3", "audio/mpeg", 128),
            SourceFile("ogg", "audio/ogg", 160), // may be Vorbis: can't tell
            SourceFile(null, null, 160),
            opusFile(null),
            opusFile(0),
            null,
        )) assertEquals("$file", opus128, opus128.forSource(file))
        assertEquals(StreamQuality.ORIGINAL, opus128.forSource(SourceFile("ogg", "audio/ogg; codecs=opus", 160)))
        assertEquals(StreamQuality.ORIGINAL, opus128.forSource(SourceFile("OPUS", null, 160)))
        assertEquals(256, opus128.opusUnchangedUpTo)
        assertNull(StreamQuality.ORIGINAL.opusUnchangedUpTo)
    }

    @Test
    fun theOriginalOnWifiIsUnaffected() {
        for (file in listOf(opusFile(180), opusFile(900), SourceFile("flac", "audio/flac", 900), null)) {
            assertEquals(StreamQuality.ORIGINAL, SourcePicker.pick("s", NetworkKind.Unmetered, QualitySettings(), file) { false })
        }
    }

    @Test
    fun aWholeCopyInTheChosenQualityBeatsStreamingTheOriginal() {
        val file = opusFile(180)
        val opus128 = setOf(CacheKeys.song("s", StreamQuality.opus(128)))
        // Mobile data, Opus 128: the rule says original, but opus128 is complete here: no data spent.
        assertEquals(StreamQuality.opus(128), SourcePicker.pick("s", NetworkKind.Metered, QualitySettings(), file) { it in opus128 })
        // Only part of it cached: the original streams, as the rule says.
        assertEquals(StreamQuality.ORIGINAL, SourcePicker.pick("s", NetworkKind.Metered, QualitySettings(), file) { false })
        // A whole original still wins over it.
        val both = opus128 + CacheKeys.song("s", StreamQuality.ORIGINAL)
        assertEquals(StreamQuality.ORIGINAL, SourcePicker.pick("s", NetworkKind.Metered, QualitySettings(), file) { it in both })
        // An Opus quality on Wi-Fi follows the same rule; a copy in another Opus quality doesn't count.
        val wifiOpus = QualitySettings(unmetered = StreamQuality.opus(192))
        val opus192 = setOf(CacheKeys.song("s", StreamQuality.opus(192)))
        assertEquals(StreamQuality.opus(192), SourcePicker.pick("s", NetworkKind.Unmetered, wifiOpus, opusFile(320)) { it in opus192 })
        assertEquals(StreamQuality.ORIGINAL, SourcePicker.pick("s", NetworkKind.Unmetered, wifiOpus, opusFile(320)) { it in opus128 })
        // A file the rule leaves transcoded is unaffected.
        assertEquals(StreamQuality.opus(128), SourcePicker.pick("s", NetworkKind.Metered, QualitySettings(), opusFile(900)) { false })
    }

    @Test
    fun storedCopiesStillComeBeforeTheOpusRule() {
        val file = opusFile(180)
        // A download in whatever quality wins.
        val downloaded = setOf(CacheKeys.song("s", StreamQuality.opus(128)))
        for (n in NetworkKind.entries) {
            assertEquals(StreamQuality.opus(128), SourcePicker.pick("s", n, QualitySettings(), file, isDownloaded = { it in downloaded }) { false })
        }
        // A whole original in the cache wins (it is also what the rule asks for).
        val original = setOf(CacheKeys.song("s", StreamQuality.ORIGINAL))
        assertEquals(StreamQuality.ORIGINAL, SourcePicker.pick("s", NetworkKind.Metered, QualitySettings(), opusFile(900)) { it in original })
        // Offline, an older opus128 copy of the same song still plays.
        val old = setOf(CacheKeys.song("s", StreamQuality.opus(128)))
        assertEquals(StreamQuality.opus(128), SourcePicker.pick("s", NetworkKind.None, QualitySettings(), file) { it in old })
        // Offline with nothing stored: the wanted quality, now the original.
        assertEquals(StreamQuality.ORIGINAL, SourcePicker.pick("s", NetworkKind.None, QualitySettings(), file) { false })
    }

    // ---- ReplayGain ---------------------------------------------------------------------

    private fun near(expected: Double, actual: Float) = assertTrue("expected $expected, got $actual", abs(expected - actual) < 1e-4)

    @Test
    fun gainInDbBecomesLinearVolume() {
        near(1.0, ReplayGainMath.volume(0.0, null))
        near(0.5011872, ReplayGainMath.volume(-6.0, null))
        near(0.1, ReplayGainMath.volume(-20.0, 0.5))
        assertEquals(1f, ReplayGainMath.volume(null, 0.9))
    }

    @Test
    fun volumeNeverAmplifiesAndKeepsThePeakUnderFullScale() {
        assertEquals(1f, ReplayGainMath.volume(+4.0, 0.5))
        // -1 dB is 0.891; a peak of 1.25 allows only 0.8.
        near(0.8, ReplayGainMath.volume(-1.0, 1.25))
        // The peak cap does not apply when it is not the limit.
        near(ReplayGainMath.dbToLinear(-8.0), ReplayGainMath.volume(-8.0, 0.99))
    }

    @Test
    fun modesPickTheRightGain() {
        val rg = ReplayGain(trackGain = -7.0, trackPeak = 0.9, albumGain = -9.0, albumPeak = 1.0)
        assertNull(ReplayGainMath.select(ReplayGainMode.Off, rg, albumInOrder = true))
        assertEquals(-7.0 to 0.9, ReplayGainMath.select(ReplayGainMode.Track, rg, albumInOrder = true))
        assertEquals(-9.0 to 1.0, ReplayGainMath.select(ReplayGainMode.Album, rg, albumInOrder = true))
        assertEquals(-7.0 to 0.9, ReplayGainMath.select(ReplayGainMode.Album, rg, albumInOrder = false))
        // Absent values: nothing, never a guess.
        assertNull(ReplayGainMath.select(ReplayGainMode.Track, null, albumInOrder = false))
        assertNull(ReplayGainMath.select(ReplayGainMode.Track, ReplayGain(albumGain = -3.0), albumInOrder = false))
        assertEquals(-3.0 to null, ReplayGainMath.select(ReplayGainMode.Album, ReplayGain(albumGain = -3.0), albumInOrder = true))
        assertEquals(1f, ReplayGainMath.volumeFor(ReplayGainMode.Track, null, albumInOrder = false))
    }

    @Test
    fun albumGainOnlyWhenTheAlbumPlaysInOrder() {
        val album = listOf(QueueEntry("A", 1, 1), QueueEntry("A", 1, 2), QueueEntry("A", 2, 1))
        assertTrue(ReplayGainMath.albumInOrder(album, 0, shuffle = false))
        assertTrue(ReplayGainMath.albumInOrder(album, 2, shuffle = false))
        assertFalse(ReplayGainMath.albumInOrder(album, 1, shuffle = true))
        val mixed = listOf(QueueEntry("A", 1, 3), QueueEntry("B", 1, 1), QueueEntry("A", 1, 5))
        assertFalse(ReplayGainMath.albumInOrder(mixed, 0, shuffle = false))
        assertFalse(ReplayGainMath.albumInOrder(mixed, 2, shuffle = false))
        val reversed = listOf(QueueEntry("A", 1, 2), QueueEntry("A", 1, 1))
        assertFalse(ReplayGainMath.albumInOrder(reversed, 0, shuffle = false))
        assertFalse(ReplayGainMath.albumInOrder(listOf(QueueEntry("A", 1, 1)), 0, shuffle = false))
    }

    // ---- Scrobbling ---------------------------------------------------------------------

    @Test
    fun scrobbleThresholdIs75PercentOrFourMinutes() {
        assertEquals(150_000L, ScrobbleRule.thresholdMs(200_000))
        assertEquals(240_000L, ScrobbleRule.thresholdMs(600_000))
        assertEquals(240_000L, ScrobbleRule.thresholdMs(null))
        assertNull(ScrobbleRule.thresholdMs(30_000))
        assertFalse(ScrobbleRule.shouldSubmit(149_999, 200_000))
        assertTrue(ScrobbleRule.shouldSubmit(150_000, 200_000))
        assertTrue(ScrobbleRule.shouldSubmit(240_000, 3_600_000))
        assertFalse(ScrobbleRule.shouldSubmit(29_000, 30_000))
    }

    private class Sent(val id: String, val time: Long, val submission: Boolean)

    private fun TestScope.scrobbler(sent: MutableList<Sent>) = Scrobbler(
        scope = this,
        clock = { testScheduler.currentTime },
        send = { id, t, sub -> sent += Sent(id, t, sub) },
    )

    @Test
    fun nowPlayingAtStartAndOneSubmissionAfterTheThreshold() = runTest {
        val sent = mutableListOf<Sent>()
        val s = scrobbler(sent)
        s.onTrackStarted("a", 200_000, playing = true, wallClockMs = 1_000)
        runCurrent()
        assertEquals(1, sent.size)
        assertFalse(sent[0].submission)
        advanceTimeBy(149_000); runCurrent()
        assertEquals(1, sent.size)
        advanceTimeBy(1_001); runCurrent()
        assertEquals(2, sent.size)
        assertTrue(sent[1].submission)
        assertEquals(1_000L, sent[1].time)
        advanceUntilIdle()
        s.onPlayingChanged(false); s.onPlayingChanged(true); advanceUntilIdle()
        assertEquals(2, sent.size) // submitted once only
    }

    @Test
    fun pausedTimeDoesNotCount() = runTest {
        val sent = mutableListOf<Sent>()
        val s = scrobbler(sent)
        s.onTrackStarted("a", 200_000, playing = true, wallClockMs = 0)
        advanceTimeBy(100_000); runCurrent()
        s.onPlayingChanged(false)
        advanceTimeBy(500_000); runCurrent()
        assertEquals(1, sent.size)
        s.onPlayingChanged(true)
        advanceTimeBy(49_000); runCurrent()
        assertEquals(1, sent.size)
        advanceTimeBy(1_001); runCurrent()
        assertEquals(2, sent.size)
    }

    @Test
    fun skippedTrackIsNotSubmitted() = runTest {
        val sent = mutableListOf<Sent>()
        val s = scrobbler(sent)
        s.onTrackStarted("a", 200_000, playing = true, wallClockMs = 0)
        advanceTimeBy(60_000); runCurrent()
        s.onTrackStarted("b", 200_000, playing = true, wallClockMs = 60_000)
        advanceTimeBy(100_000); runCurrent()
        assertEquals(listOf("a" to false, "b" to false), sent.map { it.id to it.submission })
    }

    @Test
    fun trackEndingAtThresholdBeforeTimerRunsIsStillSubmitted() = runTest {
        val sent = mutableListOf<Sent>()
        val s = scrobbler(sent)
        s.onTrackStarted("a", 200_000, playing = true, wallClockMs = 1_000)
        runCurrent()
        // Clock reaches the 150 s threshold, and the next track starts before the timer coroutine runs.
        advanceTimeBy(150_000)
        s.onTrackStarted("b", 200_000, playing = true, wallClockMs = 151_000)
        runCurrent()
        assertEquals(
            listOf("a" to false, "a" to true, "b" to false),
            sent.map { it.id to it.submission },
        )
        assertEquals(1_000L, sent[1].time)
    }

    @Test
    fun sameSongTwiceInARowInTheQueueCountsAsTwoPlays() = runTest {
        val sent = mutableListOf<Sent>()
        val s = scrobbler(sent)
        // Queue restored paused sends nothing until play starts.
        s.onItem("a#1", "a", 200_000, playWhenReady = false, isPlaying = false, wallClockMs = 0)
        runCurrent()
        assertTrue(sent.isEmpty())

        // First queue entry plays past the threshold, then transitions to a second entry of the same song.
        s.onItem("a#1", "a", 200_000, playWhenReady = true, isPlaying = true, wallClockMs = 1_000)
        runCurrent()
        advanceTimeBy(150_000)
        s.onItem("a#2", "a", 200_000, playWhenReady = true, isPlaying = true, wallClockMs = 151_000)
        runCurrent()
        advanceTimeBy(150_000); runCurrent()

        assertEquals(
            listOf(
                Triple("a", 1_000L, false),
                Triple("a", 1_000L, true),
                Triple("a", 151_000L, false),
                Triple("a", 151_000L, true),
            ),
            sent.map { Triple(it.id, it.time, it.submission) },
        )
    }

    @Test
    fun repeatingTheSameQueueEntryStartsAFreshPlay() = runTest {
        val sent = mutableListOf<Sent>()
        val s = scrobbler(sent)
        s.onItem("a#1", "a", 200_000, playWhenReady = true, isPlaying = true, wallClockMs = 1_000)
        runCurrent()
        advanceTimeBy(150_000)
        s.onRepeat()
        s.onItem("a#1", "a", 200_000, playWhenReady = true, isPlaying = true, wallClockMs = 201_000)
        runCurrent()
        advanceTimeBy(150_000); runCurrent()
        assertEquals(
            listOf(
                Triple("a", 1_000L, false),
                Triple("a", 1_000L, true),
                Triple("a", 201_000L, false),
                Triple("a", 201_000L, true),
            ),
            sent.map { Triple(it.id, it.time, it.submission) },
        )
    }

    @Test
    fun scrobbleIsRefusedInReadOnlyModeAndNeverSent() = runTest {
        val server = MockWebServer()
        server.start()
        try {
            server.enqueue(MockResponse().setBody("""{"subsonic-response":{"status":"ok","version":"1.16.1"}}"""))
            val client = SubsonicClient(SubsonicCredentials(server.url("/").toString(), "u", "s", "t"), readOnly = true)
            val logs = mutableListOf<String>()
            val s = Scrobbler(this, { testScheduler.currentTime }, client::scrobble, log = { logs += it })
            s.onTrackStarted("a", 60_000, playing = true, wallClockMs = 0)
            advanceUntilIdle()
            assertEquals(0, server.requestCount)
            assertEquals(2, logs.size)
            assertTrue(logs.all { it.contains("refused") })
        } finally {
            server.shutdown()
        }
    }

    @Test
    fun favouriteIsRefusedInReadOnlyModeAndNeverSent() = runTest {
        val server = MockWebServer()
        server.start()
        try {
            val client = SubsonicClient(SubsonicCredentials(server.url("/").toString(), "u", "s", "t"), readOnly = true)
            for (call in listOf<suspend () -> Unit>({ client.star(id = "a") }, { client.unstar(id = "a") })) {
                try {
                    call()
                    fail("expected a refusal")
                } catch (e: SubsonicError.RefusedReadOnly) {
                    // expected
                }
            }
            assertEquals(0, server.requestCount)
        } finally {
            server.shutdown()
        }
    }

    // ---- Sleep timer ----------------------------------------------------------------------

    @Test
    fun sleepTimerFadesOverTheLastTenSecondsThenPauses() = runTest {
        val fades = mutableListOf<Pair<Long, Float>>()
        var expiredAt = -1L
        val t = SleepTimer(this, { testScheduler.currentTime }, { fades += testScheduler.currentTime to it }, { expiredAt = testScheduler.currentTime })
        t.setMinutes(2)
        assertTrue(t.mode.value is SleepMode.At)
        advanceTimeBy(109_999); runCurrent()
        assertTrue("no fade before the last 10 s", fades.isEmpty())
        advanceTimeBy(1); runCurrent()
        assertEquals(110_000L to 1f, fades.first())
        advanceTimeBy(5_000); runCurrent()
        val mid = fades.last()
        assertEquals(115_000L, mid.first)
        assertEquals(0.5f, mid.second, 0.001f)
        assertEquals(-1L, expiredAt)
        advanceUntilIdle()
        assertEquals(120_000L, expiredAt)
        assertEquals(1f, fades.last().second) // volume restored after the pause
        assertEquals(SleepMode.Off, t.mode.value)
        // Steps are every 250 ms, never tighter, and only during the fade.
        assertTrue(fades.size in 40..42)
    }

    @Test
    fun cancellingTheSleepTimerRestoresVolume() = runTest {
        val fades = mutableListOf<Float>()
        var expired = false
        val t = SleepTimer(this, { testScheduler.currentTime }, { fades += it }, { expired = true })
        t.setMinutes(1)
        advanceTimeBy(55_000); runCurrent()
        t.cancel()
        advanceUntilIdle()
        assertFalse(expired)
        assertEquals(1f, fades.last())
        assertEquals(SleepMode.Off, t.mode.value)
        t.setEndOfTrack()
        assertEquals(SleepMode.EndOfTrack, t.mode.value)
    }

    @Test
    fun fadeFactorIsLinear() {
        assertEquals(1f, SleepTimer.fadeFactor(0, 20_000))
        assertEquals(0.25f, SleepTimer.fadeFactor(17_500, 20_000), 0.0001f)
        assertEquals(0f, SleepTimer.fadeFactor(20_000, 20_000))
    }

    // ---- Saved queue ------------------------------------------------------------------

    @Test
    fun savedQueueRoundTripsAndClamps() {
        val songs = listOf(
            Song(id = "1", title = "One", albumId = "A", duration = 200, replayGain = ReplayGain(trackGain = -6.5), size = 9, bitRate = 900),
            Song(id = "2", title = "Two"),
        )
        val q = SavedQueue(songs.map { it.forQueue() }, index = 1, positionMs = 42_000, shuffle = true)
        val back = SavedQueue.decode(q.encode())!!
        assertEquals(q, back)
        assertNull(back.songs[0].size) // trimmed
        assertEquals(900, back.songs[0].bitRate) // kept: the Opus rule needs it
        assertEquals(-6.5, back.songs[0].replayGain!!.trackGain!!, 0.0)
        assertEquals(1, SavedQueue.decode(q.copy(index = 7).encode())!!.index)
        assertNull(SavedQueue.decode("not json"))
        assertNull(SavedQueue.decode(SavedQueue(emptyList(), 0, 0).encode()))
    }
}
