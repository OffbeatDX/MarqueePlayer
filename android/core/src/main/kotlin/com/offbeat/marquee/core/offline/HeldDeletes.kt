package com.offbeat.marquee.core.offline

/**
 * Downloads the player is reading are not deleted under it. Removing a download (or a playlist
 * dropping a song) while that song plays, or is next and already opened for gapless playback,
 * would take its file away mid-read: offline, the track then fails. Those songs are held back
 * ([release]) and deleted once they leave the play window ([setInUse]), unless pinned again meanwhile.
 */
class HeldDeletes {
    private var inUse: Set<String> = emptySet()
    private val held = LinkedHashSet<String>()

    /** Songs to delete: returns those that can go now; the ones in use are held. */
    @Synchronized
    fun release(ids: Set<String>): Set<String> {
        val now = ids.filterTo(LinkedHashSet()) { it !in inUse }
        held += ids.filter { it in inUse }
        return now
    }

    /**
     * The songs the player is on now (the current one and the next). Returns the held songs that
     * left and are not [wanted] again: to delete now.
     */
    @Synchronized
    fun setInUse(ids: Set<String>, wanted: (String) -> Boolean): Set<String> {
        inUse = ids
        val left = held.filterTo(LinkedHashSet()) { it !in ids }
        held -= left
        return left.filterNotTo(LinkedHashSet(), wanted)
    }

    @get:Synchronized
    val heldIds: Set<String> get() = HashSet(held)
}
