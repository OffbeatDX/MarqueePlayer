package com.offbeat.marquee.player.art

import android.content.Context
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.MusicNote
import androidx.compose.material3.Icon
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Shape
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import coil3.ImageLoader
import coil3.PlatformContext
import coil3.compose.AsyncImage
import coil3.disk.DiskCache
import coil3.intercept.Interceptor
import coil3.memory.MemoryCache
import coil3.network.okhttp.OkHttpNetworkFetcherFactory
import coil3.request.CachePolicy
import coil3.request.ImageRequest
import coil3.request.ImageResult
import coil3.request.crossfade
import coil3.size.pxOrElse
import com.offbeat.marquee.core.art.CoverSize
import com.offbeat.marquee.player.offline.CoverStore
import com.offbeat.marquee.player.session.AppSession
import com.offbeat.marquee.player.ui.theme.Palette
import okhttp3.Call
import okio.Path.Companion.toOkioPath
import java.io.IOException

/**
 * Covers: Coil on the session's OkHttp client, so every image request goes through the same
 * connection pool and, in debug builds, the read-only guard. Always `getCoverArt` with a size bucket
 * ([com.offbeat.marquee.core.art.CoverSize]); cached by cover id and size, never by URL (the URL
 * carries the token). A downloaded item's covers are read from the download's own store first
 * ([CoverStore]), which no cache eviction touches.
 *
 * [CoverImage] asks for a [CoverRef]; [CoverSizing] turns it into the `getCoverArt` request for the
 * smallest bucket at least as large as the size the image is actually drawn at, in pixels.
 */
object Images {
    private const val DISK_CACHE_BYTES = 256L * 1024 * 1024

    /** Requests go through the signed-in client; with nobody signed in they fail without leaving the device. */
    private val sessionCalls = Call.Factory { request ->
        val client = AppSession.current ?: throw IOException("Not signed in")
        client.httpClient.newCall(request)
    }

    fun diskCacheDir(context: Context) = context.cacheDir.resolve("covers")

    fun newLoader(context: PlatformContext): ImageLoader = ImageLoader.Builder(context)
        .components {
            add(CoverSizing)
            add(OkHttpNetworkFetcherFactory(callFactory = { sessionCalls }))
        }
        .diskCache {
            DiskCache.Builder().directory(diskCacheDir(context).toOkioPath()).maxSizeBytes(DISK_CACHE_BYTES).build()
        }
        .memoryCache { MemoryCache.Builder().maxSizePercent(context, 0.2).build() }
        .crossfade(150)
        .build()

    fun diskKey(coverId: String, size: Int) = "cover/$coverId/$size"
    fun memoryKey(coverId: String, size: Int) = MemoryCache.Key("cover:$coverId:$size")

    /** A cover at a size bucket; null when signed out. */
    fun request(context: Context, coverId: String, size: Int): ImageRequest? =
        source(coverId, size)?.let { ImageRequest.Builder(context).sourced(it, coverId, size).build() }

    /**
     * Where a cover at [size] comes from: a download's stored cover ([CoverStore]) when it has one
     * that answers, else `getCoverArt` (through the disk cache). Null when signed out.
     */
    fun source(coverId: String, size: Int): Any? =
        CoverStore.file(coverId, size) ?: AppSession.current?.coverArtUrl(coverId, size)

    /** The keys for [source]: a stored file is read where it is, not copied into the disk cache. */
    fun ImageRequest.Builder.sourced(source: Any, coverId: String, size: Int): ImageRequest.Builder =
        data(source)
            .memoryCacheKey(memoryKey(coverId, size))
            .apply {
                if (source is java.io.File) diskCachePolicy(CachePolicy.DISABLED) else diskCacheKey(diskKey(coverId, size))
            }
}

/** A cover to draw, sized by where it is drawn ([CoverSizing]). */
data class CoverRef(val coverId: String)

/**
 * Turns a [CoverRef] into the request for the bucket that covers the drawn size: Coil has measured
 * the image by now ([Interceptor.Chain.size], from the layout's constraints), so no extra layout
 * pass or recomposition is needed. Cached by cover id and bucket.
 */
private object CoverSizing : Interceptor {
    override suspend fun intercept(chain: Interceptor.Chain): ImageResult {
        val ref = chain.request.data as? CoverRef ?: return chain.proceed()
        val px = maxOf(chain.size.width.pxOrElse { 0 }, chain.size.height.pxOrElse { 0 })
        val bucket = if (px > 0) CoverSize.bucketFor(px) else CoverSize.GRID
        val source = Images.source(ref.coverId, bucket) ?: throw IOException("Not signed in")
        val request = with(Images) { chain.request.newBuilder().sourced(source, ref.coverId, bucket) }.build()
        return chain.withRequest(request).proceed()
    }
}

/**
 * A cover clipped to [shape], asked for at the size it is drawn ([CoverSizing]), with a quiet
 * placeholder while it loads or when there is no art.
 */
@Composable
fun CoverImage(
    coverId: String?,
    modifier: Modifier = Modifier,
    shape: Shape = RoundedCornerShape(8.dp),
    contentDescription: String? = null,
) {
    val context = LocalContext.current
    val palette = Palette.current
    Box(modifier.clip(shape).background(palette.surface), contentAlignment = Alignment.Center) {
        val request = remember(coverId) {
            coverId?.takeIf { AppSession.current != null }?.let { ImageRequest.Builder(context).data(CoverRef(it)).build() }
        }
        // No art, or none to be had (offline and not in the cover cache): the desktop's placeholder,
        // its note glyph faint on the surface, instead of an empty box.
        var failed by remember(coverId) { mutableStateOf(false) }
        if (request == null || failed) {
            Icon(
                Icons.Rounded.MusicNote,
                contentDescription = contentDescription,
                tint = Color(0x38FFFFFF),
                modifier = Modifier.fillMaxSize(0.3f),
            )
        }
        if (request != null) {
            AsyncImage(
                model = request,
                contentDescription = if (failed) null else contentDescription,
                contentScale = ContentScale.Crop,
                onError = { failed = true },
                onSuccess = { failed = false },
                modifier = Modifier.fillMaxSize(),
            )
        }
    }
}
