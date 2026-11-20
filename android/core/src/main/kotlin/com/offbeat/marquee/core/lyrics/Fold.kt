package com.offbeat.marquee.core.lyrics

import java.text.Normalizer

/**
 * Names folded the way the desktop's search folds them (`library::fold`), so a search result is
 * taken only when it is really the same artist and title:
 * - full-width Latin letters, digits and punctuation as normal width, half-width katakana as
 *   full-width (NFKC does both, voicing marks included);
 * - katakana as hiragana;
 * - Latin letters without their diacritics ("Beyoncé" is "beyonce", "Dvořák" is "dvorak", "Straße"
 *   is "strasse"); only Latin, so the kana voicing marks stay (が is not か);
 * - lowercase, runs of white space made one, trimmed.
 */
object Fold {
    private val LATIN_SPECIAL = mapOf(
        'ß' to "ss", 'æ' to "ae", 'Æ' to "ae", 'ø' to "o", 'Ø' to "o", 'œ' to "oe", 'Œ' to "oe",
        'đ' to "d", 'Đ' to "d", 'ð' to "d", 'Ð' to "d", 'ł' to "l", 'Ł' to "l", 'þ' to "th", 'Þ' to "th",
        'ħ' to "h", 'Ħ' to "h", 'ı' to "i", 'ĸ' to "k", 'ŀ' to "l", 'Ŀ' to "l", 'ŉ' to "n", 'ŧ' to "t", 'Ŧ' to "t",
        'ĳ' to "ij", 'Ĳ' to "ij", 'ſ' to "s",
    )

    fun fold(s: String): String {
        val nfkc = Normalizer.normalize(s, Normalizer.Form.NFKC)
        val decomposed = Normalizer.normalize(nfkc, Normalizer.Form.NFD)
        val out = StringBuilder(decomposed.length)
        for (c in decomposed) {
            when {
                // Combining diacritics after a Latin letter go (the kana marks are U+3099/U+309A, not here).
                c in '̀'..'ͯ' -> if (out.isEmpty() || !isLatinLetter(out[out.length - 1])) out.append(c)
                c in 'ァ'..'ヶ' -> out.append(c - 0x60) // katakana -> hiragana
                else -> {
                    val special = LATIN_SPECIAL[c]
                    if (special != null) out.append(special) else out.append(c.lowercaseChar())
                }
            }
        }
        // Recompose what is left (kana with their voicing marks), then the white space.
        val recomposed = Normalizer.normalize(out, Normalizer.Form.NFC)
        return recomposed.trim().split(Regex("\\s+")).filter { it.isNotEmpty() }.joinToString(" ")
    }

    private fun isLatinLetter(c: Char): Boolean = c in 'a'..'z' || c in 'A'..'Z'
}
