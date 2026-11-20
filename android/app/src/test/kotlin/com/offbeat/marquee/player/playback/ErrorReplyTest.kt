package com.offbeat.marquee.player.playback

import com.offbeat.marquee.core.playback.PlaybackRecovery.FailureKind
import com.offbeat.marquee.core.subsonic.SubsonicClient
import com.offbeat.marquee.core.subsonic.SubsonicCredentials
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.mockwebserver.MockResponse
import okhttp3.mockwebserver.MockWebServer
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.fail
import org.junit.Before
import org.junit.Test
import java.io.IOException

/** The stream client refuses a Subsonic error answered with HTTP 200 before anything reads it. */
class ErrorReplyTest {
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

    private fun stream(): ByteArray {
        val http = StreamHttp.clientFor(client)
        http.newCall(Request.Builder().url(client.streamUrl("s1")).build()).execute().use { r ->
            return r.body!!.bytes()
        }
    }

    private fun expectReply(): ErrorReplyException {
        val e = try {
            stream()
            null
        } catch (e: IOException) {
            e
        } ?: fail("the error answer was handed on as audio").let { throw AssertionError() }
        val reply = ErrorReplyException.findIn(e)
        assertNotNull("failed for another reason: $e", reply)
        return reply!!
    }

    @Test
    fun aJsonErrorWithStatus200IsRefusedWithItsCode() {
        server.enqueue(
            MockResponse().setHeader("Content-Type", "application/json")
                .setBody("""{"subsonic-response":{"status":"failed","version":"1.16.1","error":{"code":40,"message":"Wrong username or password"}}}"""),
        )
        val e = expectReply()
        assertEquals(40, e.code)
        assertEquals(FailureKind.Auth, e.kind)
    }

    @Test
    fun anXmlNotFoundIsRefusedAsNotFound() {
        server.enqueue(
            MockResponse().setHeader("Content-Type", "text/xml; charset=utf-8")
                .setBody("""<subsonic-response status="failed"><error code="70" message="Song not found"/></subsonic-response>"""),
        )
        val e = expectReply()
        assertEquals(70, e.code)
        assertEquals(FailureKind.NotFound, e.kind)
    }

    @Test
    fun anHtmlPageIsRefusedWithoutACode() {
        server.enqueue(MockResponse().setHeader("Content-Type", "text/html").setBody("<html>Proxy login</html>"))
        val e = expectReply()
        assertEquals(null, e.code)
        assertEquals(FailureKind.Other, e.kind)
    }

    @Test
    fun audioPassesThrough() {
        server.enqueue(MockResponse().setHeader("Content-Type", "audio/flac").setBody("fLaC and the rest"))
        assertEquals("fLaC and the rest", String(stream()))
        // Without the short timeout too (the debug comparison).
        server.enqueue(MockResponse().setHeader("Content-Type", "audio/ogg").setBody("OggS"))
        StreamHttp.clientFor(client, short = false).newCall(Request.Builder().url(client.streamUrl("s1")).build()).execute().use {
            assertEquals("OggS", it.body!!.string())
        }
    }
}
