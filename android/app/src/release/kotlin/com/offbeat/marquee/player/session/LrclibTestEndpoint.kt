package com.offbeat.marquee.player.session

import android.content.Context

/** Release builds use the real LRCLIB (BuildConfig.LRCLIB_URL); see the debug source set. */
object LrclibTestEndpoint {
    @Suppress("UNUSED_PARAMETER", "FunctionOnlyReturningConstant")
    fun load(context: Context): String? = null
}
