package com.offbeat.marquee.player.playback

import com.offbeat.marquee.core.subsonic.SubsonicClient
import com.offbeat.marquee.core.subsonic.SubsonicCredentials
import okhttp3.Interceptor
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.Protocol
import okhttp3.Request
import okhttp3.Response
import okhttp3.ResponseBody.Companion.toResponseBody
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test
import java.net.ProtocolException

class StreamHttpTest {
    private fun fakeClient(respond: (Request) -> Response): SubsonicClient {
        val terminal = Interceptor { chain -> respond(chain.request()) }
        return SubsonicClient(
            SubsonicCredentials("https://music.example.com", "alice", "c19b2d", "26719a1196d2a940705a59634eb18eab"),
            readOnly = true,
            httpClient = SubsonicClient.defaultHttpClient().newBuilder().addInterceptor(terminal).build(),
        )
    }

    private fun redirectResponse(req: Request, location: String) = Response.Builder()
        .request(req)
        .protocol(Protocol.HTTP_1_1)
        .code(302)
        .message("Found")
        .header("Location", location)
        .body("".toResponseBody("text/plain".toMediaType()))
        .build()

    private fun okResponse(req: Request) = Response.Builder()
        .request(req)
        .protocol(Protocol.HTTP_1_1)
        .code(200)
        .message("OK")
        .body("audio".toResponseBody("audio/ogg".toMediaType()))
        .build()

    @Test
    fun media3StreamingClientInheritsSameOriginRedirectPolicy() {
        val seen = mutableListOf<String>()
        val client = fakeClient { req ->
            seen += req.url.toString()
            when {
                req.url.encodedPath.endsWith("/other-host") -> redirectResponse(req, "https://evil.example.com/rest/stream.view?id=1")
                req.url.encodedPath.endsWith("/downgrade") -> redirectResponse(req, "http://music.example.com/rest/stream.view?id=1")
                req.url.encodedPath == "/rest/stream.view" -> redirectResponse(req, "/v2/rest/stream.view?id=1")
                else -> okResponse(req)
            }
        }

        val streamClient = StreamHttp.clientFor(client)
        assertFalse(streamClient.followRedirects)
        assertFalse(streamClient.followSslRedirects)

        // Same-origin redirect succeeds.
        seen.clear()
        streamClient.newCall(Request.Builder().url(client.streamUrl("s1")).build()).execute().use { r ->
            assertEquals(200, r.code)
        }
        assertEquals(2, seen.size)
        assertTrue(seen.all { it.startsWith("https://music.example.com/") })

        // Redirect to another host is refused; the target is never contacted.
        seen.clear()
        val otherHostReq = Request.Builder().url("https://music.example.com/rest/stream.view?id=1").build()
        val crossHostClient = StreamHttp.clientFor(
            fakeClient { req ->
                seen += req.url.toString()
                redirectResponse(req, "https://evil.example.com/rest/stream.view?id=1&t=secret")
            },
        )
        try {
            crossHostClient.newCall(otherHostReq).execute().close()
            fail("cross-host redirect was followed")
        } catch (e: ProtocolException) {
            // expected
        }
        assertEquals(1, seen.size)
        assertTrue(seen.none { "evil.example.com" in it })

        // Redirect from https to http is refused; http is never contacted.
        seen.clear()
        val downgradeClient = StreamHttp.clientFor(
            fakeClient { req ->
                seen += req.url.toString()
                redirectResponse(req, "http://music.example.com/rest/stream.view?id=1&t=secret")
            },
        )
        try {
            downgradeClient.newCall(otherHostReq).execute().close()
            fail("https-to-http redirect was followed")
        } catch (e: ProtocolException) {
            // expected
        }
        assertEquals(1, seen.size)
        assertTrue(seen.none { it.startsWith("http://") })
    }
}
