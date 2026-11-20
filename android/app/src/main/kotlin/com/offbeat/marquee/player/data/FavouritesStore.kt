package com.offbeat.marquee.player.data

import com.offbeat.marquee.core.library.Overrides
import com.offbeat.marquee.core.library.SafeFile
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.serialization.builtins.MapSerializer
import kotlinx.serialization.builtins.serializer
import kotlinx.serialization.json.Json
import java.io.File

/**
 * The favourite states changed on this device, kept in `favourites.json` so the hearts are right
 * after a restart. Every file access (the first read, each write, the sign-out delete) runs in
 * order on [scope], which must run one thing at a time, so none of it happens on the caller's
 * thread and an older write never lands after a newer one ([FavouritesWriter]).
 *
 * [load] reads the file in the background: a change made before it finishes is newer and wins
 * ([Overrides.fillMissing]), and the file is then rewritten with both. A sign-out ([clear]) while
 * it reads makes the late result be dropped.
 */
internal class FavouritesStore(private val file: () -> File, private val scope: CoroutineScope) {
    private val overrides = Overrides<Boolean>()
    private val lock = Any()
    private val writer = FavouritesWriter()
    /** Goes up at each [clear]: a load started before it is out of date. Guarded by [lock]. */
    private var epoch = 0L
    private val _state = MutableStateFlow<Map<String, Boolean>>(emptyMap())

    val state: StateFlow<Map<String, Boolean>> = _state.asStateFlow()

    fun snapshot(): Map<String, Boolean> = overrides.snapshot()

    /** Sets one state and saves it; the ticket undoes it with [rollback]. */
    fun set(id: String, value: Boolean): Overrides.Ticket<Boolean> = overrides.set(id, value).also { publish() }

    /** Sets several states (the server's answer) and saves them once. */
    fun setAll(changes: Map<String, Boolean>) {
        if (changes.isEmpty()) return
        for ((id, v) in changes) overrides.set(id, v)
        publish()
    }

    fun rollback(ticket: Overrides.Ticket<Boolean>) {
        overrides.rollback(ticket)
        publish()
    }

    /** Starts reading what an earlier run kept; returns at once. */
    fun load(): Job {
        val startEpoch = synchronized(lock) { epoch }
        return scope.launch {
            val f = file()
            val loaded = SafeFile.readOrNull(f)?.let { runCatching { json.decodeFromString(serializer, it) }.getOrNull() }
            if (loaded.isNullOrEmpty()) return@launch
            val snap: Map<String, Boolean>
            val seq: Long
            synchronized(lock) {
                if (epoch != startEpoch || !overrides.fillMissing(loaded)) return@launch
                snap = overrides.snapshot()
                seq = writer.nextSeq()
                _state.value = snap
            }
            // Written here, on the writer's thread, before any write queued by an earlier change:
            // those carry older numbers and are skipped, so the file never loses the loaded states.
            runCatching { writer.write(f, json.encodeToString(serializer, snap), seq) }
        }
    }

    private fun publish() {
        val snap: Map<String, Boolean>
        val seq: Long
        synchronized(lock) {
            snap = overrides.snapshot()
            seq = writer.nextSeq()
            _state.value = snap
        }
        scope.launch { runCatching { writer.write(file(), json.encodeToString(serializer, snap), seq) } }
    }

    /**
     * Forgets every state and deletes the file through [writer], with a number of its own: a write
     * queued before (a toggle just before sign-out) is then skipped and can't bring the file back.
     */
    suspend fun clear() {
        val seq: Long
        synchronized(lock) {
            epoch++
            overrides.clear()
            seq = writer.nextSeq()
            _state.value = emptyMap()
        }
        scope.launch { runCatching { writer.clear(file(), seq) } }.join()
    }

    private companion object {
        val json = Json { ignoreUnknownKeys = true }
        val serializer = MapSerializer(String.serializer(), Boolean.serializer())
    }
}
