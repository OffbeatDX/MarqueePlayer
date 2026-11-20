package com.offbeat.marquee.player.settings

import android.content.Context
import android.content.SharedPreferences
import com.offbeat.marquee.core.library.AlbumSort
import com.offbeat.marquee.core.library.RecentSearches
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.serialization.builtins.ListSerializer
import kotlinx.serialization.builtins.serializer
import kotlinx.serialization.json.Json

/** What the Library and Search remember between runs: the tab, the album sort and the last searches. */
object LibraryPrefs {
    private lateinit var prefs: SharedPreferences
    private val strings = ListSerializer(String.serializer())

    private val _tab = MutableStateFlow(0)
    val tab: StateFlow<Int> = _tab.asStateFlow()

    private val _sort = MutableStateFlow(AlbumSort.DEFAULT)
    val sort: StateFlow<AlbumSort> = _sort.asStateFlow()

    private val _recent = MutableStateFlow<List<String>>(emptyList())

    /** Kept on this device only. */
    val recent: StateFlow<List<String>> = _recent.asStateFlow()

    fun init(context: Context) {
        prefs = context.applicationContext.getSharedPreferences("library", Context.MODE_PRIVATE)
        _tab.value = prefs.getInt(KEY_TAB, 0)
        _sort.value = AlbumSort.fromKey(prefs.getString(KEY_SORT, null))
        _recent.value = prefs.getString(KEY_RECENT, null)
            ?.let { runCatching { Json.decodeFromString(strings, it) }.getOrNull() }
            .orEmpty()
    }

    fun setTab(index: Int) {
        _tab.value = index
        prefs.edit().putInt(KEY_TAB, index).apply()
    }

    fun setSort(sort: AlbumSort) {
        _sort.value = sort
        prefs.edit().putString(KEY_SORT, sort.key).apply()
    }

    fun addRecent(query: String) = saveRecent(RecentSearches.add(_recent.value, query))

    fun removeRecent(query: String) = saveRecent(RecentSearches.remove(_recent.value, query))

    private fun saveRecent(list: List<String>) {
        if (list == _recent.value) return
        _recent.value = list
        prefs.edit().putString(KEY_RECENT, Json.encodeToString(strings, list)).apply()
    }

    /** Sign out: the searches go with the account. */
    fun clearRecent() = saveRecent(emptyList())

    private const val KEY_TAB = "tab"
    private const val KEY_SORT = "album_sort"
    private const val KEY_RECENT = "recent_searches"
}
