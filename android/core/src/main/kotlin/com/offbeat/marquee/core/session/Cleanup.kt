package com.offbeat.marquee.core.session

import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/**
 * Sign-out's cleanup: steps run one after the other in a scope that lives as long as the process
 * ([scope]), not as long as a screen, and none of them can be cancelled half way (deleting several GB
 * of downloads takes seconds; leaving the screen then must not leave the old account's data behind).
 * A step that fails is logged and the next one still runs. A second cleanup waits for the first.
 *
 * A new sign-in calls [awaitIdle] first, so nothing of the new account is deleted by a cleanup still
 * running for the old one.
 */
class Cleanup(private val scope: CoroutineScope, private val log: (String, Throwable?) -> Unit = { _, _ -> }) {
    private var last: Job? = null

    /** Runs [steps] (name to step), in order, after any cleanup still running. */
    @Synchronized
    fun start(steps: List<Pair<String, suspend () -> Unit>>): Job {
        val before = last
        val job = scope.launch {
            withContext(NonCancellable) {
                before?.join()
                for ((name, step) in steps) {
                    try {
                        step()
                    } catch (e: CancellationException) {
                        log("sign-out: $name interrupted", e)
                    } catch (e: Exception) {
                        log("sign-out: $name failed", e)
                    }
                }
            }
        }
        last = job
        return job
    }

    /** Returns once no cleanup is running. */
    suspend fun awaitIdle() {
        val job = synchronized(this) { last } ?: return
        job.join()
    }

    val running: Boolean get() = synchronized(this) { last?.isActive == true }
}
