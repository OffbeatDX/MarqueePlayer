package com.offbeat.marquee.player.session

import android.content.Context
import com.offbeat.marquee.core.subsonic.SubsonicClient
import com.offbeat.marquee.core.subsonic.SubsonicError
import com.offbeat.marquee.player.BuildConfig
import com.offbeat.marquee.player.diag.AppLog
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

/**
 * The signed-in server for the whole process: the UI and the playback service share one client,
 * so they share one OkHttp client (one connection pool) and, in read-only builds, its
 * [com.offbeat.marquee.core.subsonic.ReadOnlyInterceptor]. Every network path in the app goes
 * through [SubsonicClient.httpClient].
 */
object AppSession {
    private val _client = MutableStateFlow<SubsonicClient?>(null)
    val client: StateFlow<SubsonicClient?> = _client.asStateFlow()

    @Volatile private var loaded = false

    val current: SubsonicClient? get() = _client.value

    /** Signed in (by the UI). [testServer] and debug builds are always read-only. */
    fun set(client: SubsonicClient) {
        AppLog.hide(client.credentials)
        loaded = true
        _client.value = client
    }

    fun clear() {
        loaded = true
        _client.value = null
    }

    /**
     * The client, loading the stored session first when nothing has set one yet (the service can
     * start before any UI, e.g. from a Bluetooth play button). Blocking: call it off the main thread.
     */
    @Synchronized
    fun ensureLoaded(context: Context): SubsonicClient? {
        if (loaded) return _client.value
        val app = context.applicationContext
        val test = TestServerConfig.load(app)
        val creds = test ?: SessionStore(app).load()
        val c = creds?.let {
            try {
                SubsonicClient(it, readOnly = test != null || BuildConfig.READ_ONLY)
            } catch (e: SubsonicError) {
                null
            }
        }
        c?.let { AppLog.hide(it.credentials) }
        loaded = true
        if (_client.value == null) _client.value = c
        return _client.value
    }
}
