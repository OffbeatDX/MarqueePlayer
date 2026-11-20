package com.offbeat.marquee.core.playback

/**
 * Seeking in a track the player can't seek in.
 *
 * A live transcode (an Opus quality the server encodes on the fly) comes with no length and no
 * ranges, so the player reads it as an unseekable stream for the whole time it plays: a seek does
 * nothing, or goes back to the start. The prefetcher fetches the track whole within seconds, and
 * once it is complete on the phone its length is known, so the same track loaded again from there
 * seeks like any file. So a seek in an unseekable track becomes a reload of the track from the
 * stored copy, at the position asked for ([Kind.Reload]); while the track isn't complete yet, the
 * latest position is kept and the reload happens as soon as it is ([Kind.WaitForTrack], then
 * [recheck]). A newer seek replaces the one waiting; another track, or a seek to one, drops it.
 *
 * [Item.key] tells queue entries apart (song id and entry), so a pending seek never lands on
 * another entry of the same song.
 */
class SeekReload {
    enum class Kind {
        /** Hand the seek to the player: it can do it (or nothing is loaded yet, and it starts there). */
        Seek,

        /** Load the track again from the stored copy, starting at the position. */
        Reload,

        /** Keep the position until the whole track is on the phone (say so: this can take seconds). */
        WaitForTrack,

        /** Keep the position until the player knows whether the track seeks (it is opening it). */
        WaitForSource,

        /** A reload from the stored copy didn't make it seekable either: nothing to do. */
        Ignore,
    }

    data class Action(val kind: Kind, val positionMs: Long)

    /**
     * The current entry as the player sees it: [prepared] (not idle), [placeholder] (opening: whether
     * it seeks isn't known yet), [seekable], and [stored] (complete on the phone in the quality it
     * plays in, with a known length).
     */
    data class Item(
        val key: String,
        val prepared: Boolean,
        val placeholder: Boolean,
        val seekable: Boolean,
        val stored: Boolean,
    )

    private data class Pending(val key: String, val positionMs: Long)

    private var pending: Pending? = null
    /** The entry being loaded again: until it is ready its timeline is the old, unseekable one. */
    private var reloading: String? = null
    /** The entry that was loaded again from its stored copy and still couldn't seek. */
    private var failed: String? = null

    /** The position waiting to be sought to, if any. */
    val pendingMs: Long? get() = pending?.positionMs

    /** A seek inside the current entry. */
    fun seek(item: Item, positionMs: Long): Action {
        val target = positionMs.coerceAtLeast(0)
        val kind = when {
            !item.prepared -> Kind.Seek
            item.seekable && !item.placeholder -> Kind.Seek
            item.key == failed -> Kind.Ignore
            item.placeholder || item.key == reloading -> Kind.WaitForSource
            item.stored -> Kind.Reload
            // Back to the start works on any stream: the player reads it from the top again.
            target == 0L -> Kind.Seek
            else -> Kind.WaitForTrack
        }
        pending = if (kind == Kind.WaitForTrack || kind == Kind.WaitForSource) Pending(item.key, target) else null
        if (kind == Kind.Reload) reloading = item.key
        return Action(kind, target)
    }

    /**
     * Something changed (the player learned whether the entry seeks, it became ready, the track was
     * fetched or downloaded): the seek waiting, if it can be done now. Null: nothing to do yet.
     */
    fun recheck(item: Item): Action? {
        val p = pending ?: return null
        if (p.key != item.key) {
            pending = null
            return null
        }
        if (!item.prepared || item.placeholder || item.key == reloading) return null
        if (item.seekable) {
            pending = null
            return Action(Kind.Seek, p.positionMs)
        }
        if (item.key == failed) {
            pending = null
            return null
        }
        if (!item.stored) {
            if (p.positionMs != 0L) return null
            pending = null
            return Action(Kind.Seek, 0)
        }
        pending = null
        reloading = item.key
        return Action(Kind.Reload, p.positionMs)
    }

    /**
     * The entry is ready to play (call [recheck] after it). After a reload its seek map is known now;
     * true when it is still unseekable: the stored copy can't seek either, and it isn't tried again.
     */
    fun onReady(item: Item): Boolean {
        if (item.key != reloading) return false
        reloading = null
        if (!item.seekable) {
            failed = item.key
            return true
        }
        return false
    }

    /** Another entry plays now, or a seek went to another one: whatever waited for this one goes. */
    fun onOtherItem() {
        pending = null
        reloading = null
        failed = null
    }
}
