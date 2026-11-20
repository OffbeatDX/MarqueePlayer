package com.offbeat.marquee.player.art

import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import com.offbeat.marquee.player.diag.AppLog
import android.util.LruCache
import androidx.compose.animation.Crossfade
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.FilterQuality
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import coil3.SingletonImageLoader
import coil3.request.CachePolicy
import coil3.request.ImageRequest
import coil3.request.SuccessResult
import coil3.request.allowHardware
import coil3.size.Precision
import coil3.toBitmap
import com.offbeat.marquee.core.art.Accent
import com.offbeat.marquee.core.art.Blur
import com.offbeat.marquee.core.art.CoverSize
import com.offbeat.marquee.core.library.DiskStore
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.io.File
import java.util.concurrent.ConcurrentHashMap

/**
 * What is computed from a cover once and kept: its accent colour and its blurred backdrop. Both
 * come from one small decoded image (64 px, read from the grid-size cover so the disk cache is
 * shared), worked on off the main thread, and cached by cover id in memory and on disk. Never a
 * live blur.
 */
object Art {
    private const val TAG = "MarqueeArt"
    private const val BLUR_RADIUS = 4

    private lateinit var app: Context
    private val accents = ConcurrentHashMap<String, Int>()
    private val backdrops = LruCache<String, ImageBitmap>(24)
    private val inflight = ConcurrentHashMap<String, CompletableDeferred<Unit>>()

    fun init(context: Context) {
        app = context.applicationContext
    }

    private fun accentDir() = File(app.filesDir, "art/accent")
    private fun backdropDir() = File(app.cacheDir, "art/backdrop")
    private fun name(coverId: String) = DiskStore.safeName(coverId)

    /** What is already known in memory, for a first frame without a flash. */
    fun knownAccent(coverId: String?): Int? = if (coverId == null) Accent.FALLBACK else accents[coverId]
    fun knownBackdrop(coverId: String?): ImageBitmap? = coverId?.let { backdrops.get(it) }

    /** The accent (0xRRGGBB) for a cover: [Accent.FALLBACK] when there is none or it is greyscale. */
    suspend fun accent(coverId: String?): Int {
        if (coverId == null) return Accent.FALLBACK
        accents[coverId]?.let { return it }
        withContext(Dispatchers.IO) {
            val f = File(accentDir(), name(coverId) + ".txt")
            if (f.isFile) runCatching { f.readText() }.getOrNull()?.let { accents[coverId] = Accent.parse(it) ?: Accent.FALLBACK }
        }
        accents[coverId]?.let { return it }
        compute(coverId)
        return accents[coverId] ?: Accent.FALLBACK
    }

    /** The blurred backdrop (a small image, drawn scaled up under a tint), or null without art. */
    suspend fun backdrop(coverId: String): ImageBitmap? {
        backdrops.get(coverId)?.let { return it }
        val fromDisk = withContext(Dispatchers.IO) {
            val f = File(backdropDir(), name(coverId) + ".png")
            // Capped again: backdrops saved before the cap existed are brought down to it (a no-op since).
            if (f.isFile) runCatching { BitmapFactory.decodeFile(f.path)?.let(::capped)?.asImageBitmap() }.getOrNull() else null
        }
        if (fromDisk != null) {
            backdrops.put(coverId, fromDisk)
            return fromDisk
        }
        compute(coverId)
        return backdrops.get(coverId)
    }

    /** Decodes the sample once and makes both the accent and the backdrop; concurrent callers wait for it. */
    private suspend fun compute(coverId: String) {
        val mine = CompletableDeferred<Unit>()
        val running = inflight.putIfAbsent(coverId, mine)
        if (running != null) {
            running.await()
            return
        }
        try {
            val sample = loadSample(coverId) ?: return
            withContext(Dispatchers.Default) {
                val w = sample.width
                val h = sample.height
                val pixels = IntArray(w * h)
                sample.getPixels(pixels, 0, w, 0, 0, w, h)
                val accent = Accent.forPixels(pixels)
                accents[coverId] = accent
                // Bright covers are brought down so dim text stays readable over them.
                val blurred = Blur.capBrightness(Blur.boxBlur(pixels.copyOf(), w, h, BLUR_RADIUS))
                val bmp = Bitmap.createBitmap(blurred, w, h, Bitmap.Config.ARGB_8888)
                backdrops.put(coverId, bmp.asImageBitmap())
                withContext(Dispatchers.IO) {
                    runCatching {
                        accentDir().mkdirs()
                        File(accentDir(), name(coverId) + ".txt").writeText(Accent.format(accent))
                        backdropDir().mkdirs()
                        File(backdropDir(), name(coverId) + ".png").outputStream().use { bmp.compress(Bitmap.CompressFormat.PNG, 100, it) }
                    }
                }
            }
        } finally {
            inflight.remove(coverId)
            mine.complete(Unit)
        }
    }

    /** [bmp] with [Blur.capBrightness] applied (a small image: about 64 px square). */
    private fun capped(bmp: Bitmap): Bitmap {
        val w = bmp.width
        val h = bmp.height
        val px = IntArray(w * h)
        bmp.getPixels(px, 0, w, 0, 0, w, h)
        return Bitmap.createBitmap(Blur.capBrightness(px), w, h, Bitmap.Config.ARGB_8888)
    }

    private suspend fun loadSample(coverId: String): Bitmap? {
        val source = Images.source(coverId, CoverSize.THUMB) ?: return null
        val request = with(Images) { ImageRequest.Builder(app).sourced(source, coverId, CoverSize.THUMB) }
            // Not the grid's decoded image: a separate, uncached 64 px decode.
            .memoryCachePolicy(CachePolicy.DISABLED)
            .size(CoverSize.SAMPLE, CoverSize.SAMPLE)
            .precision(Precision.EXACT)
            .allowHardware(false)
            .build()
        val result = SingletonImageLoader.get(app).execute(request)
        if (result !is SuccessResult) {
            AppLog.i(TAG, "no sample for a cover (${result::class.simpleName})")
            return null
        }
        val bmp = result.image.toBitmap()
        return if (bmp.width == CoverSize.SAMPLE && bmp.height == CoverSize.SAMPLE) bmp
        else Bitmap.createScaledBitmap(bmp, CoverSize.SAMPLE, CoverSize.SAMPLE, true)
    }

    /** Forgets everything (sign out). */
    fun clear() {
        accents.clear()
        backdrops.evictAll()
        accentDir().deleteRecursively()
        backdropDir().deleteRecursively()
    }
}

/** The blurred backdrop of a cover, null until it is ready (or without art). */
@Composable
fun rememberBackdrop(coverId: String?): ImageBitmap? {
    val state = produceState(Art.knownBackdrop(coverId), coverId) {
        value = coverId?.let { Art.backdrop(it) }
    }
    return state.value
}

/**
 * A cover's blurred backdrop filling the space, under [tint]; crossfades when the cover changes
 * (600 ms, as the desktop's). Drawn scaled up from the small blurred image.
 */
@Composable
fun Backdrop(coverId: String?, tint: Color, modifier: Modifier = Modifier) {
    val bitmap = rememberBackdrop(coverId)
    Box(modifier) {
        Crossfade(targetState = bitmap, animationSpec = tween(600), label = "backdrop") { b ->
            if (b != null) {
                Image(
                    bitmap = b,
                    contentDescription = null,
                    contentScale = ContentScale.Crop,
                    filterQuality = FilterQuality.Low,
                    modifier = Modifier.fillMaxSize(),
                )
            }
        }
        Box(Modifier.fillMaxSize().background(tint))
    }
}

/**
 * The accent for the playing cover, moving to a new one over 600 ms in OKLCH (hue the short way
 * round), as the desktop does. [Accent.FALLBACK] without a cover or for greyscale art; [fixed]
 * instead when the user chose one.
 */
@Composable
fun animatedAccent(coverId: String?, fixed: Int? = null): Int {
    val cover = if (fixed != null) null else coverId
    val fromCover by produceState(Art.knownAccent(cover) ?: Accent.FALLBACK, cover) { value = Art.accent(cover) }
    // A fixed accent (Settings > Appearance) wins; switching between the two glides like a new cover.
    val target = fixed ?: fromCover
    var from by remember { mutableIntStateOf(target) }
    var to by remember { mutableIntStateOf(target) }
    val t = remember { Animatable(1f) }
    LaunchedEffect(target) {
        if (target == to) return@LaunchedEffect
        from = Accent.mix(from, to, t.value)
        to = target
        t.snapTo(0f)
        t.animateTo(1f, tween(600))
    }
    return if (t.value >= 1f) to else Accent.mix(from, to, t.value)
}
