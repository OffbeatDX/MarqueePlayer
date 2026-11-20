package com.offbeat.marquee.player.playback

import com.offbeat.marquee.core.playback.SavedQueue
import com.offbeat.marquee.core.subsonic.Song
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.io.File
import java.util.concurrent.CountDownLatch
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit

class QueueStoreTest {
    @get:Rule
    val tmp = TemporaryFolder()

    private fun queue(songId: String, positionMs: Long) = SavedQueue(
        songs = listOf(Song(id = songId, title = "Song $songId")),
        index = 0,
        positionMs = positionMs,
    )

    @Test
    fun olderSnapshotArrivingLateDoesNotOverwriteNewerSave() {
        val store = QueueStore(File(tmp.root, "queue.json"))
        val olderSeq = store.nextSeq()
        val newerSeq = store.nextSeq()
        val older = queue("old", 10_000)
        val newer = queue("new", 42_000)

        assertTrue(store.save(newer, newerSeq))
        assertFalse(store.save(older, olderSeq))
        assertEquals(newer, store.load())
    }

    @Test
    fun olderSaveAfterClearDoesNotResurrectTheQueue() {
        val store = QueueStore(File(tmp.root, "queue.json"))
        val saveSeq = store.nextSeq()
        val clearSeq = store.nextSeq()

        assertTrue(store.clear(clearSeq))
        assertFalse(store.save(queue("stale", 5_000), saveSeq))
        assertNull(store.load())
    }

    @Test
    fun olderClearAfterNewerSaveDoesNotDeleteTheQueue() {
        val store = QueueStore(File(tmp.root, "queue.json"))
        val clearSeq = store.nextSeq()
        val saveSeq = store.nextSeq()
        val latest = queue("fresh", 1_000)

        assertTrue(store.save(latest, saveSeq))
        assertFalse(store.clear(clearSeq))
        assertEquals(latest, store.load())
    }

    @Test
    fun concurrentOutOfOrderSavesKeepTheNewestSnapshot() {
        val file = File(tmp.root, "queue.json")
        val store = QueueStore(file)
        val snapshots = (1..60).map { i -> store.nextSeq() to queue("s$i", i * 1_000L) }
        val pool = Executors.newFixedThreadPool(6)
        val ready = CountDownLatch(1)
        val done = CountDownLatch(snapshots.size)
        try {
            // Submit in reverse order across threads so older snapshots race against newer ones.
            for ((seq, q) in snapshots.reversed()) {
                pool.execute {
                    ready.await()
                    store.save(q, seq)
                    done.countDown()
                }
            }
            ready.countDown()
            assertTrue(done.await(15, TimeUnit.SECONDS))
        } finally {
            pool.shutdownNow()
        }
        assertEquals(snapshots.last().second, store.load())
        assertFalse("temp file left behind", File(tmp.root, "queue.json.tmp").exists())
    }
}
