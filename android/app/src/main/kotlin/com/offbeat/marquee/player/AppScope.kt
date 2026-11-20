package com.offbeat.marquee.player

import com.offbeat.marquee.player.diag.AppLog
import kotlinx.coroutines.CoroutineExceptionHandler
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlin.coroutines.CoroutineContext

/**
 * The scope of work a screen starts but must not take down with it when it goes (rotation, Back,
 * leaving the page): pinning and removing downloads, a heart, sign-out's cleanup. It lives as long as
 * the process, on the main thread (the work moves to IO itself); a failure is logged, not a crash.
 */
object AppScope : CoroutineScope {
    override val coroutineContext: CoroutineContext = SupervisorJob() + Dispatchers.Main.immediate +
        CoroutineExceptionHandler { _, e -> AppLog.e("MarqueeAppScope", "background action failed", e) }
}
