package com.offbeat.marquee.player.session

import android.content.Context
import com.offbeat.marquee.core.subsonic.SubsonicCredentials

/** Release builds never read a pushed test config; see the debug source set. */
object TestServerConfig {
    @Suppress("UNUSED_PARAMETER", "FunctionOnlyReturningConstant")
    fun load(context: Context): SubsonicCredentials? = null

    @Suppress("UNUSED_PARAMETER")
    fun clear(context: Context) = Unit
}
