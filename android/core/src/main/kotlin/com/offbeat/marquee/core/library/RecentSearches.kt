package com.offbeat.marquee.core.library

/** The last searches, newest first: a repeat moves to the top (case ignored), at most [MAX]. */
object RecentSearches {
    const val MAX = 10

    private val spaces = Regex("\\s+")

    fun add(list: List<String>, query: String): List<String> {
        val q = query.trim().replace(spaces, " ")
        if (q.isEmpty()) return list
        return (listOf(q) + list.filterNot { it.equals(q, ignoreCase = true) }).take(MAX)
    }

    fun remove(list: List<String>, query: String): List<String> = list.filterNot { it == query }
}
