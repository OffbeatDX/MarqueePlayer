package com.offbeat.marquee.core.subsonic

import kotlinx.coroutines.test.runTest
import okhttp3.OkHttpClient
import okhttp3.mockwebserver.MockResponse
import okhttp3.mockwebserver.MockWebServer
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Before
import org.junit.Test
import java.util.concurrent.TimeUnit

/**
 * The release build's behaviour: a client that is not read-only sends every write (favourites,
 * scrobbles, playlist additions) to the server as Subsonic expects them. Test builds never do (see
 * [SubsonicClientTest.refusedWritesNeverReachTheServer]); this keeps the normal path covered.
 */
class ReadWriteClientTest {
    private lateinit var server: MockWebServer
    private lateinit var client: SubsonicClient

    @Before
    fun setUp() {
        server = MockWebServer()
        server.start()
        client = SubsonicClient(
            SubsonicCredentials(server.url("/").toString(), "alice", "c19b2d", "26719a1196d2a940705a59634eb18eab"),
            readOnly = false,
            httpClient = OkHttpClient.Builder().readTimeout(5, TimeUnit.SECONDS).build(),
        )
    }

    @After
    fun tearDown() {
        runCatching { server.shutdown() }
    }

    private fun ok() = MockResponse().setBody("""{"subsonic-response":{"status":"ok","version":"1.16.1"}}""")

    private fun next() = server.takeRequest(5, TimeUnit.SECONDS)!!.requestUrl!!

    @Test
    fun noReadOnlyGuardOnTheHttpClient() {
        assertFalse(client.readOnly)
        assertFalse(client.httpClient.interceptors.any { it is ReadOnlyInterceptor })
    }

    @Test
    fun starAndUnstarAreSent() = runTest {
        repeat(3) { server.enqueue(ok()) }
        client.star(id = "s1")
        client.star(albumId = "al1")
        client.unstar(artistId = "ar1")
        next().let {
            assertEquals("/rest/star.view", it.encodedPath)
            assertEquals("s1", it.queryParameter("id"))
            assertNull(it.queryParameter("albumId"))
            assertEquals("alice", it.queryParameter("u"))
            assertEquals("26719a1196d2a940705a59634eb18eab", it.queryParameter("t"))
        }
        next().let {
            assertEquals("/rest/star.view", it.encodedPath)
            assertEquals("al1", it.queryParameter("albumId"))
        }
        next().let {
            assertEquals("/rest/unstar.view", it.encodedPath)
            assertEquals("ar1", it.queryParameter("artistId"))
        }
        assertEquals(3, server.requestCount)
    }

    @Test
    fun scrobblesAreSentNowPlayingSubmissionAndBatched() = runTest {
        repeat(3) { server.enqueue(ok()) }
        client.scrobble("s1", timeMillis = 1_700_000_000_000, submission = false)
        client.scrobble("s1", timeMillis = 1_700_000_000_000)
        client.scrobble(listOf("s2", "s3"), listOf(1_700_000_100_000, 1_700_000_200_000))
        next().let {
            assertEquals("/rest/scrobble.view", it.encodedPath)
            assertEquals("false", it.queryParameter("submission"))
            assertEquals("1700000000000", it.queryParameter("time"))
        }
        next().let {
            assertEquals("/rest/scrobble.view", it.encodedPath)
            assertEquals("true", it.queryParameter("submission"))
            assertEquals("s1", it.queryParameter("id"))
        }
        next().let {
            assertEquals("/rest/scrobble.view", it.encodedPath)
            assertEquals(listOf("s2", "s3"), it.queryParameterValues("id"))
            assertEquals(listOf("1700000100000", "1700000200000"), it.queryParameterValues("time"))
            assertEquals("true", it.queryParameter("submission"))
        }
    }

    @Test
    fun aServerRefusalIsAnErrorNotARefusedReadOnly() = runTest {
        server.enqueue(MockResponse().setBody("""{"subsonic-response":{"status":"failed","version":"1.16.1","error":{"code":50,"message":"not allowed"}}}"""))
        try {
            client.star(id = "s1")
            throw AssertionError("expected an error")
        } catch (e: SubsonicError.Server) {
            assertEquals(50, e.code)
        }
        assertEquals(1, server.requestCount)
    }
}
