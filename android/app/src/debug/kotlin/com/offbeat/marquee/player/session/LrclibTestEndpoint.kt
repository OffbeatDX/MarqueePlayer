package com.offbeat.marquee.player.session

import android.content.Context
import okhttp3.HttpUrl.Companion.toHttpUrlOrNull
import java.io.File

/**
 * Debug and benchmark builds only: a local stand-in for LRCLIB, so its path can be seen in the UI
 * without contacting lrclib.net. `adb reverse tcp:<port> tcp:<port>` and a one-line file
 * `files/lrclib-test-url.txt` (`http://127.0.0.1:<port>`) turn it on; anything that is not the
 * loopback address is ignored. Release builds have a stub that always returns null.
 */
object LrclibTestEndpoint {
    private const val FILE = "lrclib-test-url.txt"

    fun load(context: Context): String? {
        val f = File(context.filesDir, FILE)
        if (!f.isFile) return null
        val text = runCatching { f.readText().trim() }.getOrNull() ?: return null
        val url = text.toHttpUrlOrNull() ?: return null
        return if (url.host == "127.0.0.1" || url.host == "localhost") text.trimEnd('/') else null
    }
}
