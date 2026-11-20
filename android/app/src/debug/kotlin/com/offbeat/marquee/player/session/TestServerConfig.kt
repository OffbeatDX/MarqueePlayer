package com.offbeat.marquee.player.session

import android.content.Context
import com.offbeat.marquee.core.subsonic.SubsonicCredentials
import java.io.File
import java.util.Properties

/**
 * Debug builds only: the test server pushed by `tools/emulator.ps1 push-test-config` into the
 * app's private files. When it is there the app signs in with it and is read-only, whatever the
 * file says. Release builds have a stub that always returns null.
 */
object TestServerConfig {
    private const val FILE = "test-server.properties"

    fun load(context: Context): SubsonicCredentials? {
        val file = File(context.filesDir, FILE)
        if (!file.isFile) return null
        val p = Properties().apply { runCatching { file.inputStream().use { load(it) } } }
        val url = p.getProperty("url")?.trim().orEmpty()
        val user = p.getProperty("user")?.trim().orEmpty()
        val salt = p.getProperty("salt")?.trim().orEmpty()
        val token = p.getProperty("token")?.trim().orEmpty()
        if (url.isEmpty() || user.isEmpty() || salt.isEmpty() || token.isEmpty()) return null
        return SubsonicCredentials(url, user, salt, token)
    }

    /** Sign out removes it, so the next start shows the sign-in screen. */
    fun clear(context: Context) {
        File(context.filesDir, FILE).delete()
    }
}
