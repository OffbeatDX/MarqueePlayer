package com.offbeat.marquee.player.settings

import android.content.Context
import android.content.SharedPreferences
import com.offbeat.marquee.core.settings.AccentChoice
import com.offbeat.marquee.core.settings.ShelfLayout
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

/**
 * Look, Home and lyrics settings, in SharedPreferences `ui`: pure black for OLED, the accent (from
 * the cover or fixed), Home's shelves (order and visibility), "Look up lyrics online" (on by default;
 * test builds have no LRCLIB, so it does nothing there), whether Now Playing showed the lyrics last
 * time, and the server's name and version as last seen (Settings > Account, About).
 */
object UiSettings {
    private lateinit var prefs: SharedPreferences
    private val _pureBlack = MutableStateFlow(false)
    val pureBlack: StateFlow<Boolean> = _pureBlack.asStateFlow()
    private val _accent = MutableStateFlow<AccentChoice>(AccentChoice.FromCover)
    val accent: StateFlow<AccentChoice> = _accent.asStateFlow()
    private val _shelves = MutableStateFlow(ShelfLayout.DEFAULT)
    val shelves: StateFlow<ShelfLayout> = _shelves.asStateFlow()
    private val _lyricsOnline = MutableStateFlow(true)
    val lyricsOnline: StateFlow<Boolean> = _lyricsOnline.asStateFlow()
    private val _lyricsOpen = MutableStateFlow(false)
    val lyricsOpen: StateFlow<Boolean> = _lyricsOpen.asStateFlow()
    private val _server = MutableStateFlow<String?>(null)

    /** "Navidrome 0.64.2" (or the API version for another server), as the last ping answered; null before one. */
    val server: StateFlow<String?> = _server.asStateFlow()

    fun init(context: Context) {
        prefs = context.applicationContext.getSharedPreferences("ui", Context.MODE_PRIVATE)
        _pureBlack.value = prefs.getBoolean(KEY_PURE_BLACK, false)
        _accent.value = AccentChoice.decode(prefs.getString(KEY_ACCENT, null))
        _shelves.value = ShelfLayout.decode(prefs.getString(KEY_SHELVES, null))
        _lyricsOnline.value = prefs.getBoolean(KEY_LYRICS_ONLINE, true)
        _lyricsOpen.value = prefs.getBoolean(KEY_LYRICS_OPEN, false)
        _server.value = prefs.getString(KEY_SERVER, null)
    }

    fun setPureBlack(on: Boolean) {
        _pureBlack.value = on
        prefs.edit().putBoolean(KEY_PURE_BLACK, on).apply()
    }

    fun setAccent(choice: AccentChoice) {
        _accent.value = choice
        prefs.edit().putString(KEY_ACCENT, choice.encode()).apply()
    }

    fun setShelves(layout: ShelfLayout) {
        if (layout == _shelves.value) return
        _shelves.value = layout
        prefs.edit().putString(KEY_SHELVES, layout.encode()).apply()
    }

    fun setLyricsOnline(on: Boolean) {
        _lyricsOnline.value = on
        prefs.edit().putBoolean(KEY_LYRICS_ONLINE, on).apply()
    }

    fun setLyricsOpen(open: Boolean) {
        _lyricsOpen.value = open
        prefs.edit().putBoolean(KEY_LYRICS_OPEN, open).apply()
    }

    fun setServer(description: String?) {
        if (description == _server.value) return
        _server.value = description
        prefs.edit().putString(KEY_SERVER, description).apply()
    }

    private const val KEY_PURE_BLACK = "pure_black"
    private const val KEY_ACCENT = "accent"
    private const val KEY_SHELVES = "shelves"
    private const val KEY_LYRICS_ONLINE = "lyrics_online"
    private const val KEY_LYRICS_OPEN = "lyrics_open"
    private const val KEY_SERVER = "server"
}
