package com.offbeat.marquee.core.diag

/**
 * The text file "Share diagnostics" builds: titled sections of `name: value` rows or free text
 * (the last crash, the log). [render] scrubs the whole of it once more, whatever each part was.
 */
class DiagnosticReport(private val title: String) {
    private val parts = mutableListOf<String>()

    fun section(name: String, rows: List<Pair<String, String>>): DiagnosticReport = apply {
        val width = rows.maxOfOrNull { it.first.length } ?: 0
        parts += buildString {
            append(heading(name))
            for ((k, v) in rows) append(k.padEnd(width)).append("  ").append(v).append('\n')
        }
    }

    fun text(name: String, body: String?): DiagnosticReport = apply {
        parts += heading(name) + (body?.trimEnd()?.takeIf { it.isNotEmpty() } ?: "(none)") + "\n"
    }

    fun render(scrubber: Scrubber): String = scrubber.scrub(
        buildString {
            append(title).append('\n')
            for (p in parts) append('\n').append(p)
        },
    )

    private fun heading(name: String) = "== $name ==\n"
}
