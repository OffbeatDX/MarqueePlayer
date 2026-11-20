package com.offbeat.marquee.core.settings

import com.offbeat.marquee.core.art.Accent
import com.offbeat.marquee.core.library.ShelfId
import com.offbeat.marquee.core.library.Shelves
import com.offbeat.marquee.core.playback.StreamQuality

/**
 * Home's shelves as the user arranged them (Settings > Home > Edit shelves): every shelf in an
 * order, some hidden. Home and Android Auto show [visible]. At least one shelf stays visible.
 */
data class ShelfLayout(val order: List<ShelfId>, val hidden: Set<ShelfId> = emptySet()) {
    init {
        require(order.toSet().size == order.size) { "a shelf is listed twice" }
    }

    /** What Home shows, top to bottom. */
    val visible: List<ShelfId> get() = order.filter { it !in hidden }

    val isDefault: Boolean get() = this == DEFAULT

    /** [id] moved to [to] (an index into [order], clamped). */
    fun move(id: ShelfId, to: Int): ShelfLayout {
        val from = order.indexOf(id)
        if (from < 0) return this
        val target = to.coerceIn(0, order.lastIndex)
        if (target == from) return this
        val list = order.toMutableList()
        list.removeAt(from)
        list.add(target, id)
        return copy(order = list)
    }

    /** Shown or hidden; hiding the last visible shelf is refused (Home would be empty). */
    fun setShown(id: ShelfId, shown: Boolean): ShelfLayout = when {
        shown -> copy(hidden = hidden - id)
        visible == listOf(id) -> this
        else -> copy(hidden = hidden + id)
    }

    fun canHide(id: ShelfId): Boolean = id in hidden || visible.size > 1

    /** "recently_added,!random,…": the order, hidden ones marked with `!`. */
    fun encode(): String = order.joinToString(",") { (if (it in hidden) "!" else "") + it.key }

    companion object {
        val DEFAULT = ShelfLayout(Shelves.DEFAULT)

        /**
         * Back from [encode]. Unknown names are dropped; a shelf the stored text doesn't mention (one
         * added in a later version) goes in at its place in the default order, shown. Garbage or
         * everything hidden gives the default.
         */
        fun decode(text: String?): ShelfLayout {
            if (text.isNullOrBlank()) return DEFAULT
            val order = mutableListOf<ShelfId>()
            val hidden = mutableSetOf<ShelfId>()
            for (part in text.split(',')) {
                val p = part.trim()
                val isHidden = p.startsWith("!")
                val id = ShelfId.fromKey(p.removePrefix("!")) ?: continue
                if (id in order) continue
                order += id
                if (isHidden) hidden += id
            }
            if (order.isEmpty()) return DEFAULT
            for ((i, id) in Shelves.DEFAULT.withIndex()) {
                if (id in order) continue
                // After the shelf that comes before it by default, or first.
                val before = Shelves.DEFAULT.take(i).lastOrNull { it in order }
                order.add(if (before == null) 0 else order.indexOf(before) + 1, id)
            }
            val layout = ShelfLayout(order, hidden)
            return if (layout.visible.isEmpty()) layout.copy(hidden = emptySet()) else layout
        }
    }
}

/** The cache's size cap (Settings > Cache) and the decision of what goes when it is lowered. */
object CacheCap {
    private const val GB = 1024L * 1024 * 1024
    val CHOICES_GB: List<Int> = listOf(1, 2, 4, 8)
    const val DEFAULT_GB = 2

    fun bytes(gb: Int): Long = gb * GB

    /** A stored cap back to one of [CHOICES_GB] (the nearest at or below it; the default for nonsense). */
    fun gbOf(bytes: Long): Int {
        if (bytes <= 0) return DEFAULT_GB
        return CHOICES_GB.lastOrNull { bytes(it) <= bytes } ?: CHOICES_GB.first()
    }

    /**
     * How many of the cache's spans to evict, oldest first ([sizesOldestFirst]), so that [used] plus
     * [incoming] fits under [cap]: the least recently used go until it fits, nothing when it does.
     * The cache keeps at least what is being written when that alone is over the cap.
     */
    fun evictions(sizesOldestFirst: List<Long>, used: Long, cap: Long, incoming: Long = 0): Int {
        var total = used
        var n = 0
        while (total + incoming > cap && n < sizesOldestFirst.size) {
            total -= sizesOldestFirst[n]
            n++
        }
        return n
    }
}

/** How many tracks after the current one are fetched whole ahead of time (Settings > Cache). */
object PrefetchAhead {
    val RANGE: IntRange = 0..5
    const val DEFAULT = 2

    fun clamp(n: Int): Int = n.coerceIn(RANGE)
}

/** The streaming and download qualities offered in Settings, with their labels. */
object QualityChoices {
    val OFFERED: List<StreamQuality> = listOf(
        StreamQuality.ORIGINAL,
        StreamQuality.opus(320),
        StreamQuality.opus(192),
        StreamQuality.opus(128),
        StreamQuality.opus(96),
    )

    fun label(q: StreamQuality): String = if (q.isOriginal) "Original" else "Opus ${q.maxBitRate}"

    /** Short, for a segmented control on a phone: "Original", "320", "192"… */
    fun shortLabel(q: StreamQuality): String = if (q.isOriginal) "Original" else "${q.maxBitRate}"

    /** A stored quality that is no longer offered shows as the offered one nearest in bitrate. */
    fun nearestOffered(q: StreamQuality): StreamQuality {
        if (q in OFFERED) return q
        if (q.isOriginal) return StreamQuality.ORIGINAL
        val kbps = q.maxBitRate ?: return StreamQuality.ORIGINAL
        return OFFERED.filter { !it.isOriginal }.minBy { kotlin.math.abs((it.maxBitRate ?: 0) - kbps) }
    }
}

/**
 * Where the accent comes from (Settings > Appearance): the playing cover (the desktop's way), or a
 * fixed colour from [SWATCHES]. Every swatch is already a usable accent (clamped as cover accents are).
 */
sealed interface AccentChoice {
    data object FromCover : AccentChoice
    data class Fixed(val rgb: Int) : AccentChoice

    /** The accent to draw with, given the playing cover's. */
    fun effective(coverAccent: Int): Int = when (this) {
        FromCover -> coverAccent
        is Fixed -> rgb
    }

    fun encode(): String = when (this) {
        FromCover -> "cover"
        is Fixed -> Accent.format(rgb)
    }

    companion object {
        /** The desktop's fallback first, then seven more around the hue circle. */
        private val RAW = listOf(
            Accent.FALLBACK, // lavender
            0x7EB6F2, // sky
            0x5FD0C2, // teal
            0x93D17C, // green
            0xF0C35A, // amber
            0xF2946E, // coral
            0xF27FB6, // pink
            0xF07A84, // red
        )
        val SWATCHES: List<Int> = RAW.map { Accent.usable(it).first }
        val NAMES: List<String> = listOf("Lavender", "Sky", "Teal", "Green", "Amber", "Coral", "Pink", "Red")

        fun decode(text: String?): AccentChoice {
            if (text == null || text == "cover") return FromCover
            val rgb = Accent.parse(text) ?: return FromCover
            return Fixed(rgb)
        }
    }
}
