package com.offbeat.marquee.core.session

import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.advanceUntilIdle
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class CleanupTest {
    @Test
    fun leavingTheScreenDoesNotStopTheCleanup() = runTest {
        val process = CoroutineScope(SupervisorJob() + StandardTestDispatcher(testScheduler))
        val screen = CoroutineScope(Job() + StandardTestDispatcher(testScheduler))
        val cleanup = Cleanup(process)
        val done = mutableListOf<String>()
        val gate = CompletableDeferred<Unit>()
        screen.launch {
            cleanup.start(listOf(
                "outbox" to { done += "outbox" },
                "downloads" to { gate.await(); done += "downloads" },
                "caches" to { done += "caches" },
            ))
        }
        advanceUntilIdle()
        // Back pressed on the sign-in screen while the downloads are deleted.
        screen.cancel()
        gate.complete(Unit)
        advanceUntilIdle()
        assertEquals(listOf("outbox", "downloads", "caches"), done)
    }

    @Test
    fun aFailedStepDoesNotSkipTheRest() = runTest {
        val done = mutableListOf<String>()
        val logged = mutableListOf<String>()
        val c = Cleanup(CoroutineScope(SupervisorJob() + StandardTestDispatcher(testScheduler))) { m, _ -> logged += m }
        c.start(listOf("a" to { throw IllegalStateException("disk") }, "b" to { done += "b" }))
        advanceUntilIdle()
        assertEquals(listOf("b"), done)
        assertEquals(listOf("sign-out: a failed"), logged)
        assertFalse(c.running)
    }

    @Test
    fun aNewSignInWaitsForTheCleanup() = runTest {
        val cleanup = Cleanup(CoroutineScope(SupervisorJob() + StandardTestDispatcher(testScheduler)))
        val gate = CompletableDeferred<Unit>()
        val order = mutableListOf<String>()
        cleanup.start(listOf("session store" to { gate.await(); order += "cleared" }))
        launch {
            cleanup.awaitIdle()
            order += "new session saved"
        }
        advanceUntilIdle()
        assertTrue(cleanup.running)
        assertTrue(order.isEmpty())
        gate.complete(Unit)
        advanceUntilIdle()
        assertEquals(listOf("cleared", "new session saved"), order)
    }

    @Test
    fun aSecondCleanupRunsAfterTheFirst() = runTest {
        val cleanup = Cleanup(CoroutineScope(SupervisorJob() + StandardTestDispatcher(testScheduler)))
        val gate = CompletableDeferred<Unit>()
        val order = mutableListOf<String>()
        cleanup.start(listOf("first" to { gate.await(); order += "first" }))
        cleanup.start(listOf("second" to { order += "second" }))
        advanceUntilIdle()
        gate.complete(Unit)
        advanceUntilIdle()
        assertEquals(listOf("first", "second"), order)
    }
}
