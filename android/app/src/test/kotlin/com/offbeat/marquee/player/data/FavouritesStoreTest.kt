package com.offbeat.marquee.player.data

import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.builtins.MapSerializer
import kotlinx.serialization.builtins.serializer
import kotlinx.serialization.json.Json
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.io.File
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicReference

@OptIn(ExperimentalCoroutinesApi::class)
class FavouritesStoreTest {
    @get:Rule
    val tmp = TemporaryFolder()

    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO.limitedParallelism(1))
    private val file get() = File(tmp.root, "favourites.json")
    private val serializer = MapSerializer(String.serializer(), Boolean.serializer())

    @After
    fun tearDown() = scope.cancel()

    private fun saved(): Map<String, Boolean> = Json.decodeFromString(serializer, file.readText())

    /** Holds the store's thread until the returned latch is released (the file read is "slow"). */
    private fun holdTheThread(): CountDownLatch {
        val release = CountDownLatch(1)
        val running = CountDownLatch(1)
        scope.launch {
            running.countDown()
            release.await(15, TimeUnit.SECONDS)
        }
        running.await(15, TimeUnit.SECONDS)
        return release
    }

    /** Everything queued on the store's thread so far has run. */
    private fun drain() = runBlocking { scope.launch { }.join() }

    @Test
    fun startUpReadIsNotOnTheCallerThread() {
        file.writeText("""{"a":true}""")
        val readOn = AtomicReference<Thread>()
        val store = FavouritesStore({ readOn.set(Thread.currentThread()); file }, scope)
        val job = store.load()
        runBlocking { job.join() }
        assertNotEquals(Thread.currentThread(), readOn.get())
        assertEquals(mapOf("a" to true), store.state.value)
    }

    @Test
    fun aToggleMadeBeforeTheLoadFinishesWins() {
        file.writeText("""{"a":true,"b":true}""")
        val store = FavouritesStore({ file }, scope)
        val release = holdTheThread()
        val job = store.load()
        store.set("a", false) // the user taps the heart while the file is still being read
        assertEquals(mapOf("a" to false), store.state.value)
        release.countDown()
        runBlocking { job.join() }
        drain()
        assertEquals(mapOf("a" to false, "b" to true), store.state.value)
        // The file keeps both: the toggle's own (older) snapshot did not overwrite the loaded states.
        assertEquals(mapOf("a" to false, "b" to true), saved())
    }

    @Test
    fun signOutDuringTheLoadDropsTheLateResult() {
        file.writeText("""{"a":true}""")
        val store = FavouritesStore({ file }, scope)
        val release = holdTheThread()
        val job = store.load()
        // Undispatched: clear() forgets the states here and now, then waits for the store's thread.
        val cleared = CoroutineScope(Dispatchers.Default).launch(start = CoroutineStart.UNDISPATCHED) { store.clear() }
        release.countDown()
        runBlocking {
            job.join()
            cleared.join()
        }
        drain()
        assertEquals(emptyMap<String, Boolean>(), store.state.value)
        assertFalse("the file came back after sign-out", file.exists())
    }

    @Test
    fun noFileMeansNothingLoaded() {
        val store = FavouritesStore({ file }, scope)
        runBlocking { store.load().join() }
        assertEquals(emptyMap<String, Boolean>(), store.state.value)
        assertFalse(file.exists())
    }
}
