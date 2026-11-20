package com.offbeat.marquee.player.data

import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.launch
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.io.File
import java.util.concurrent.CountDownLatch
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicReference

class FavouritesWriterTest {
    @get:Rule
    val tmp = TemporaryFolder()

    @Test
    fun olderSnapshotIsSkippedAfterNewerWrite() {
        val file = File(tmp.root, "favourites.json")
        val writer = FavouritesWriter()
        val olderSeq = writer.nextSeq()
        val newerSeq = writer.nextSeq()

        assertTrue(writer.write(file, """{"a":true,"b":false}""", newerSeq))
        assertFalse(writer.write(file, """{"a":false}""", olderSeq))
        assertEquals("""{"a":true,"b":false}""", file.readText())
    }

    /** Sign-out: a toggle's write that was queued before the clear but runs after it is skipped. */
    @Test
    fun lateWriteDoesNotBringBackAClearedFile() {
        val file = File(tmp.root, "favourites.json")
        val writer = FavouritesWriter()
        assertTrue(writer.write(file, """{"a":true}""", writer.nextSeq()))
        val queuedBeforeSignOut = writer.nextSeq()
        val signOut = writer.nextSeq()

        assertTrue(writer.clear(file, signOut))
        assertFalse(file.exists())
        assertFalse(writer.write(file, """{"a":true,"b":true}""", queuedBeforeSignOut))
        assertFalse("a late write re-created the file after sign-out", file.exists())

        // A change made after sign-out (the next sign-in) is written as usual.
        assertTrue(writer.write(file, """{"c":true}""", writer.nextSeq()))
        assertEquals("""{"c":true}""", file.readText())
    }

    @Test
    fun olderClearDoesNotDeleteANewerWrite() {
        val file = File(tmp.root, "favourites.json")
        val writer = FavouritesWriter()
        val clearSeq = writer.nextSeq()
        assertTrue(writer.write(file, """{"a":true}""", writer.nextSeq()))
        assertFalse(writer.clear(file, clearSeq))
        assertEquals("""{"a":true}""", file.readText())
    }

    @Test
    fun concurrentOutOfOrderWritesSerializeAndKeepNewestSnapshot() {
        val file = File(tmp.root, "favourites.json")
        val writer = FavouritesWriter()
        val snapshots = (1..60).map { i -> writer.nextSeq() to """{"s$i":true}""" }
        val pool = Executors.newFixedThreadPool(6)
        val ready = CountDownLatch(1)
        val done = CountDownLatch(snapshots.size)
        try {
            for ((seq, text) in snapshots.reversed()) {
                pool.execute {
                    ready.await()
                    writer.write(file, text, seq)
                    done.countDown()
                }
            }
            ready.countDown()
            assertTrue(done.await(15, TimeUnit.SECONDS))
        } finally {
            pool.shutdownNow()
        }
        assertEquals(snapshots.last().second, file.readText())
        assertFalse("temp file left behind", File(tmp.root, "favourites.json.tmp").exists())
    }

    @OptIn(ExperimentalCoroutinesApi::class)
    @Test
    fun serializedDispatcherWritesOffTheCallerThreadInOrder() {
        val file = File(tmp.root, "favourites.json")
        val writer = FavouritesWriter()
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO.limitedParallelism(1))
        val callerThread = Thread.currentThread()
        val writeThread = AtomicReference<Thread>()
        val done = CountDownLatch(2)
        try {
            val s1 = writer.nextSeq()
            val s2 = writer.nextSeq()
            scope.launch {
                writeThread.set(Thread.currentThread())
                writer.write(file, """{"a":true}""", s1)
                done.countDown()
            }
            scope.launch {
                writer.write(file, """{"a":true,"b":true}""", s2)
                done.countDown()
            }
            assertTrue(done.await(15, TimeUnit.SECONDS))
        } finally {
            scope.cancel()
        }
        assertNotEquals(callerThread, writeThread.get())
        assertEquals("""{"a":true,"b":true}""", file.readText())
    }
}
