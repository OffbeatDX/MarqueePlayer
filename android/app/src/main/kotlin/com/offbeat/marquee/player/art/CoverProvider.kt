package com.offbeat.marquee.player.art

import android.content.ContentProvider
import android.content.ContentValues
import android.content.Context
import android.content.Intent
import android.database.Cursor
import android.net.Uri
import android.os.ParcelFileDescriptor
import com.offbeat.marquee.player.diag.AppLog
import coil3.SingletonImageLoader
import coil3.request.CachePolicy
import coil3.request.SuccessResult
import com.offbeat.marquee.core.art.CoverUris
import com.offbeat.marquee.player.BuildConfig
import com.offbeat.marquee.player.net.NetworkMonitor
import com.offbeat.marquee.player.offline.CoverStore
import com.offbeat.marquee.player.session.AppSession
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeoutOrNull
import java.io.File
import java.io.FileNotFoundException

/**
 * Cover art for other apps: Android Auto, the system's media controls, anything given an item's
 * `artworkUri`. Those load the URI themselves, so it is `content://<app>.covers/cover/<id>/<size>`
 * ([CoverUris]) and never a `getCoverArt` URL with the token in it.
 *
 * Answers from Coil's disk cache (the app's one cover cache, keyed by cover id and size bucket, so a
 * cover the app drew is not fetched again), else fetches it once through the session's client into
 * that cache. Not exported: an app sees covers only after the playback service granted it read
 * access on connecting ([Covers.grantTo]). Read only.
 */
class CoverProvider : ContentProvider() {
    override fun onCreate(): Boolean = true

    override fun getType(uri: Uri): String = "image/*"

    override fun openFile(uri: Uri, mode: String): ParcelFileDescriptor {
        if (mode != "r") throw SecurityException("covers are read only")
        val req = CoverUris.parse(uri.pathSegments) ?: throw FileNotFoundException("not a cover")
        val ctx = context ?: throw FileNotFoundException("no context")
        val file = Covers.file(ctx, req) ?: throw FileNotFoundException("cover not available")
        return ParcelFileDescriptor.open(file, ParcelFileDescriptor.MODE_READ_ONLY)
    }

    override fun query(uri: Uri, projection: Array<out String>?, selection: String?, selectionArgs: Array<out String>?, sortOrder: String?): Cursor? = null
    override fun insert(uri: Uri, values: ContentValues?): Uri? = throw UnsupportedOperationException()
    override fun delete(uri: Uri, selection: String?, selectionArgs: Array<out String>?): Int = throw UnsupportedOperationException()
    override fun update(uri: Uri, values: ContentValues?, selection: String?, selectionArgs: Array<out String>?): Int = throw UnsupportedOperationException()
}

object Covers {
    private const val TAG = "MarqueeCovers"
    private const val FETCH_TIMEOUT_MS = 15_000L

    const val AUTHORITY = "${BuildConfig.APPLICATION_ID}.covers"

    /** The cover's content URI at a size bucket ([com.offbeat.marquee.core.art.CoverSize]). */
    fun uri(coverId: String, size: Int): Uri = Uri.parse(CoverUris.uri(AUTHORITY, coverId, size))

    fun coverIdOf(uri: Uri?): String? = CoverUris.coverIdOf(uri?.toString(), AUTHORITY)

    private val root: Uri = Uri.parse("content://$AUTHORITY/${CoverUris.SEGMENT}")

    /**
     * Lets another app ([packageName], a media controller that connected) read every cover URI: one
     * prefix grant, held until the device restarts. Nothing else of the app's is readable.
     */
    fun grantTo(context: Context, packageName: String) {
        if (packageName == context.packageName) return
        try {
            context.grantUriPermission(packageName, root, Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_GRANT_PREFIX_URI_PERMISSION)
            AppLog.i(TAG, "covers readable by $packageName")
        } catch (e: RuntimeException) {
            // A controller without a real package (a legacy one the system names generically).
            AppLog.i(TAG, "no cover grant for $packageName (${e.javaClass.simpleName})")
        }
    }

    /**
     * The cover's file in Coil's disk cache, fetching it first when it is missing (and the network is
     * up). Blocking: the provider calls it on a binder thread.
     */
    fun file(context: Context, req: CoverUris.Request): File? {
        val app = context.applicationContext
        val loader = SingletonImageLoader.get(app)
        val key = Images.diskKey(req.coverId, req.size)
        cached(loader, key)?.let { return it }
        // A download's own cover (any size that answers; offline, the nearest it has).
        CoverStore.file(req.coverId, req.size)?.let { return it }
        if (AppSession.ensureLoaded(app) == null || !NetworkMonitor.online) return null
        val request = Images.request(app, req.coverId, req.size)?.newBuilder()
            // Only the bytes are wanted (into the disk cache): a small decode, not kept in memory.
            ?.memoryCachePolicy(CachePolicy.DISABLED)
            ?.size(64)
            ?.build() ?: return null
        val result = runBlocking { withTimeoutOrNull(FETCH_TIMEOUT_MS) { loader.execute(request) } }
        if (result !is SuccessResult) {
            AppLog.i(TAG, "cover fetch failed (${result?.javaClass?.simpleName ?: "timeout"})")
            return null
        }
        return cached(loader, key)
    }

    private fun cached(loader: coil3.ImageLoader, key: String): File? =
        loader.diskCache?.openSnapshot(key)?.use { it.data.toFile().takeIf(File::isFile) }
}
