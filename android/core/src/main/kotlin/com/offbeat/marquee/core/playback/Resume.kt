package com.offbeat.marquee.core.playback

/**
 * Going on with a song stored in part, without stitching two files together.
 *
 * A song replaced on the server under the same id (re-tagged, re-encoded) must not be completed
 * from the new file's second half onto the old file's first. Every answer's validator is kept with
 * the stored bytes ([validatorOf]: its strong `ETag`, else its `Last-Modified`); a resumed request
 * sends it back as `If-Range`, so the server sends the rest only if the file is still that one, and
 * the whole new file (a 200) otherwise ([isChanged]): the part is then dropped and the song fetched
 * from the start. A part without a validator can't be checked, so it is fetched again ([plan]).
 * The desktop client does the same (`crates/subsonic`, `StreamHead::validator`).
 */
object Resume {
    sealed interface Plan {
        /** Nothing stored: fetch from the start. */
        data object Start : Plan
        /** Go on from what is stored, with `If-Range: [ifRange]`. */
        data class Continue(val ifRange: String) : Plan
        /** What is stored can't be checked (no validator) or is known to be stale: drop it and start again. */
        data object Restart : Plan
    }

    fun plan(storedBytes: Long, validator: String?): Plan = when {
        storedBytes <= 0 -> Plan.Start
        validator.isNullOrBlank() -> Plan.Restart
        else -> Plan.Continue(validator)
    }

    /** The validator of an answer: a strong ETag (a weak `W/` one can't be used with If-Range), else Last-Modified. */
    fun validatorOf(etag: String?, lastModified: String?): String? {
        val e = etag?.trim()
        if (!e.isNullOrEmpty() && !e.startsWith("W/")) return e
        return lastModified?.trim()?.takeIf { it.isNotEmpty() }
    }

    /** The first byte a `Range: bytes=N-` header asks for, or 0. */
    fun rangeStart(range: String?): Long {
        val r = range?.trim() ?: return 0
        if (!r.startsWith("bytes=")) return 0
        return r.removePrefix("bytes=").substringBefore('-').trim().toLongOrNull() ?: 0
    }

    /**
     * The answer to a resumed request ([rangeStart] > 0 with an `If-Range`) says the file is not the
     * one stored: a 200 (the whole file, as If-Range asks when it changed), or a part of a file whose
     * validator is not the one sent.
     */
    fun isChanged(rangeStart: Long, sentIfRange: String?, status: Int, validator: String?): Boolean {
        if (rangeStart <= 0 || sentIfRange == null) return false
        if (status == 200) return true
        return status == 206 && validator != null && validator != sentIfRange
    }
}
