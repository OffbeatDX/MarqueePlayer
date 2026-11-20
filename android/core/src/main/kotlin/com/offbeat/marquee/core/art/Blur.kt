package com.offbeat.marquee.core.art

/**
 * A software blur for the backdrops: three passes of a box blur (close to a Gaussian), done once per
 * cover on a small image (about 64 px) and then drawn scaled up. Never a live blur.
 */
object Blur {
    /** Blurs [pixels] (ARGB, row-major, [width] x [height]) in place and returns them. Alpha is kept opaque. */
    fun boxBlur(pixels: IntArray, width: Int, height: Int, radius: Int, passes: Int = 3): IntArray {
        require(pixels.size == width * height) { "pixels do not match ${width}x$height" }
        if (radius <= 0 || width == 0 || height == 0) return pixels
        val tmp = IntArray(pixels.size)
        repeat(passes) {
            pass(pixels, tmp, width, height, radius, horizontal = true)
            pass(tmp, pixels, width, height, radius, horizontal = false)
        }
        return pixels
    }

    /**
     * The brightest a backdrop channel may be, so dim text stays readable on it: under the normal
     * tint (69 % of the background) a white cover's backdrop left the dim text at 2.9:1; capped at
     * 150 it is 4.9:1 (WCAG AA), and the faint text 3.7:1. Darker covers are unchanged.
     */
    const val BACKDROP_MAX_CHANNEL = 150

    /**
     * Darkens each pixel whose brightest channel is above [max] to bring it down to [max], keeping
     * its hue and saturation (all three channels scaled alike). In place; returns [pixels].
     */
    fun capBrightness(pixels: IntArray, max: Int = BACKDROP_MAX_CHANNEL): IntArray {
        for (i in pixels.indices) {
            val p = pixels[i]
            val r = (p shr 16) and 0xFF
            val g = (p shr 8) and 0xFF
            val b = p and 0xFF
            val top = maxOf(r, g, b)
            if (top <= max) continue
            pixels[i] = (p and 0xFF000000.toInt()) or
                ((r * max / top) shl 16) or ((g * max / top) shl 8) or (b * max / top)
        }
        return pixels
    }

    /** One running-sum pass along rows or columns, edges clamped. */
    private fun pass(src: IntArray, dst: IntArray, w: Int, h: Int, r: Int, horizontal: Boolean) {
        val lines = if (horizontal) h else w
        val len = if (horizontal) w else h
        val window = 2 * r + 1
        for (line in 0 until lines) {
            fun at(i: Int): Int {
                val j = i.coerceIn(0, len - 1)
                return if (horizontal) src[line * w + j] else src[j * w + line]
            }
            var sr = 0
            var sg = 0
            var sb = 0
            for (i in -r..r) {
                val p = at(i)
                sr += (p shr 16) and 0xFF
                sg += (p shr 8) and 0xFF
                sb += p and 0xFF
            }
            for (i in 0 until len) {
                val out = (0xFF shl 24) or ((sr / window) shl 16) or ((sg / window) shl 8) or (sb / window)
                if (horizontal) dst[line * w + i] = out else dst[i * w + line] = out
                val add = at(i + r + 1)
                val drop = at(i - r)
                sr += ((add shr 16) and 0xFF) - ((drop shr 16) and 0xFF)
                sg += ((add shr 8) and 0xFF) - ((drop shr 8) and 0xFF)
                sb += (add and 0xFF) - (drop and 0xFF)
            }
        }
    }
}
