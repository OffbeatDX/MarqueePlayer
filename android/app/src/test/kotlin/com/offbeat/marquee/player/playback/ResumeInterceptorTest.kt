package com.offbeat.marquee.player.playback

import com.offbeat.marquee.core.subsonic.SubsonicClient
import com.offbeat.marquee.core.subsonic.SubsonicCredentials
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.mockwebserver.MockResponse
import okhttp3.mockwebserver.MockWebServer
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Before
import org.junit.Test

/** A resumed stream of a file replaced on the server fails instead of being stitched to the old part. */
class ResumeInterceptorTest {
    private lateinit var server: MockWebServer
    private lateinit var client: SubsonicClient

    @Before
    fun setUp() {
        server = MockWebServer()
        server.start()
        client = SubsonicClient(
            SubsonicCredentials(server.url("/").toString(), "alice", "c19b2d", "26719a1196d2a940705a59634eb18eab"),
            readOnly = true,
            httpClient = OkHttpClient.Builder().build(),
        )
    }

    @After
    fun tearDown() {
        runCatching { server.shutdown() }
    }

    private fun get(range: String? = null, ifRange: String? = null) = StreamHttp.clientFor(client).newCall(
        Request.Builder().url(client.streamUrl("s1")).apply {
            range?.let { header("Range", it) }
            ifRange?.let { header("If-Range", it) }
        }.build(),
    ).execute()

    @Test
    fun theFirstAnswersValidatorIsKept() {
        server.enqueue(MockResponse().setHeader("Content-Type", "audio/flac").setHeader("ETag", "\"v1\"").setBody("fLaC0123456789"))
        get().use { assertEquals(200, it.code) }
        assertEquals("\"v1\"", StreamHttp.validatorSeen(client.streamUrl("s1")))
    }

    @Test
    fun aWholeNewFileAnsweringTheResumeFails() {
        server.enqueue(MockResponse().setHeader("Content-Type", "audio/flac").setHeader("ETag", "\"v2\"").setBody("fLaC-the-new-file"))
        try {
            get(range = "bytes=10-", ifRange = "\"v1\"").close()
            fail("the new file was handed on to be stored after the old part")
        } catch (e: FileChangedException) {
            assertTrue(Resumable.isStale(e))
        }
        val sent = server.takeRequest()
        assertEquals("\"v1\"", sent.getHeader("If-Range"))
        assertEquals("bytes=10-", sent.getHeader("Range"))
    }

    @Test
    fun theSameFileGoesOn() {
        server.enqueue(
            MockResponse().setResponseCode(206).setHeader("Content-Type", "audio/flac").setHeader("ETag", "\"v1\"")
                .setHeader("Content-Range", "bytes 10-13/14").setBody("6789"),
        )
        get(range = "bytes=10-", ifRange = "\"v1\"").use { assertEquals(206, it.code) }
    }

    @Test
    fun aPlainRangeAnsweredWholeIsLeftToTheCaller() {
        // A live transcode has no ranges: Media3 skips ahead itself; no If-Range was sent.
        server.enqueue(MockResponse().setHeader("Content-Type", "audio/ogg").setBody("OggS..."))
        get(range = "bytes=10-").use { assertEquals(200, it.code) }
    }
}
