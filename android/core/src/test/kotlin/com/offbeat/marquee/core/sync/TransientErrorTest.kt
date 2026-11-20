package com.offbeat.marquee.core.sync

import com.offbeat.marquee.core.subsonic.SubsonicError
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** One rule decides "queue it and try later" against "the server said no", for direct writes and the outbox alike. */
class TransientErrorTest {
    private val transient = listOf(
        SubsonicError.Network(SubsonicError.Network.Kind.Timeout, null),
        SubsonicError.NotSubsonic("captive portal page"),
        SubsonicError.Server(code = null, httpStatus = 503, serverMessage = null),
        SubsonicError.Server(code = null, httpStatus = 408, serverMessage = null),
        SubsonicError.Server(code = null, httpStatus = 429, serverMessage = null),
        SubsonicError.Server(code = 0, httpStatus = null, serverMessage = "generic"),
        SubsonicError.AuthFailed(40, null),
        SubsonicError.InvalidUrl(),
    )
    private val refused = listOf(
        SubsonicError.Server(code = 70, httpStatus = null, serverMessage = "not found"),
        SubsonicError.Server(code = 50, httpStatus = null, serverMessage = "not authorized"),
        SubsonicError.RefusedReadOnly("star"),
    )

    @Test
    fun classification() {
        for (e in transient) assertTrue(e.message, e.isTransient())
        for (e in refused) assertFalse(e.message, e.isTransient())
    }

    @Test
    fun directWriteQueuesOnServerTroubleNotOnlyOnNetwork() = runTest {
        for (e in transient) {
            var queued = 0
            val r = sendOrQueue(online = true, send = { throw e }, queue = { queued++ })
            assertEquals(e.message, DirectWrite.Queued(e), r)
            assertEquals(e.message, 1, queued)
        }
    }

    @Test
    fun directWriteRefusalIsNotQueued() = runTest {
        for (e in refused) {
            var queued = 0
            val r = sendOrQueue(online = true, send = { throw e }, queue = { queued++ })
            assertEquals(DirectWrite.Refused(e), r)
            assertEquals(0, queued)
        }
    }

    @Test
    fun offlineQueuesWithoutTrying() = runTest {
        var sent = 0
        var queued = 0
        assertEquals(DirectWrite.Queued(null), sendOrQueue(online = false, send = { sent++ }, queue = { queued++ }))
        assertEquals(0, sent)
        assertEquals(1, queued)
        assertEquals(DirectWrite.Sent, sendOrQueue(online = true, send = { sent++ }, queue = { queued++ }))
        assertEquals(1, sent)
        assertEquals(1, queued)
    }

    /** The outbox keeps (and backs off on) exactly what a direct write would have queued. */
    @Test
    fun outboxAgreesWithDirectWrites() = runTest {
        for (e in transient + refused) {
            val box = Outbox(MemoryFile(), clock = { 1_000 })
            box.setStar("song", "s1", true, 1)
            val result = box.flush(object : OutboxSender {
                override suspend fun scrobble(ids: List<String>, timesMs: List<Long>) = Unit
                override suspend fun star(kind: String, id: String, starred: Boolean) = throw e
            })
            assertEquals(e.message, e.isTransient(), result is FlushResult.Retry)
            assertEquals(e.message, e.isTransient(), !box.isEmpty)
        }
    }

    private class MemoryFile : TextFile {
        var text: String? = null
        override fun read() = text
        override fun write(text: String) { this.text = text }
    }
}
