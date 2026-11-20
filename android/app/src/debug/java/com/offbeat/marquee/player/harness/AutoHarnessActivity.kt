package com.offbeat.marquee.player.harness

import android.annotation.SuppressLint
import android.app.Activity
import androidx.media3.session.legacy.MediaBrowserCompat
import android.content.ComponentName
import android.graphics.BitmapFactory
import android.media.browse.MediaBrowser
import android.media.session.MediaController
import android.media.session.PlaybackState
import android.net.Uri
import android.os.Bundle
import android.os.SystemClock
import android.service.media.MediaBrowserService
import android.util.Log
import com.offbeat.marquee.player.art.Covers
import com.offbeat.marquee.player.playback.PlaybackService
import com.offbeat.marquee.player.session.AppSession
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.MainScope
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeout
import java.io.File

/**
 * Debug builds only (not in benchmark or release): a stand-in for Android Auto. It talks to
 * [PlaybackService] the way Auto does, through the platform's legacy [MediaBrowser] and
 * [MediaController] (Media3's MediaBrowserServiceCompat side), and logs what it finds under the tag
 * `MarqueeHarness` and in `files/harness.txt`:
 *
 * - the root and its extras (search supported, content style hints);
 * - a walk of the tree: every tab, the first nodes in each, an album's tracks, paging;
 * - every artwork URI seen: a content URI of the cover provider, with no token, salt, user or host
 *   in it (checked against the signed-in credentials without printing them), and one opened;
 * - a search;
 * - playing an album item and a track item (playFromMediaId), then the session's state, queue and
 *   custom actions (the favourite heart Auto shows in its playback view).
 *
 * Ids, counts and kinds only: no titles, nothing from the account.
 *
 * `adb -s emulator-5580 shell am start -n com.offbeat.marquee.player.debug/com.offbeat.marquee.player.harness.AutoHarnessActivity`
 * (add `--es query <text>` for the search; `--ez play false` to skip playing).
 */
class AutoHarnessActivity : Activity() {
    private val scope = MainScope()
    private val out = StringBuilder()
    private var browser: MediaBrowser? = null
    private val seenArt = LinkedHashSet<String>()
    private var problems = 0

    private fun log(line: String) {
        Log.i(TAG, line)
        out.append(line).append('\n')
    }

    private fun problem(line: String) {
        problems++
        log("PROBLEM: $line")
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        // `--ez pinWidget true`: asks the launcher to add the widget to the home screen (its dialog follows).
        if (intent.getBooleanExtra("pinWidget", false)) {
            val ok = getSystemService(android.appwidget.AppWidgetManager::class.java)
                .requestPinAppWidget(ComponentName(this, com.offbeat.marquee.player.widget.PlayerWidgetReceiver::class.java), null, null)
            log("pin widget requested: $ok")
            finish()
            return
        }
        val query = intent.getStringExtra("query")
        val play = intent.getBooleanExtra("play", true)
        scope.launch {
            val started = SystemClock.elapsedRealtime()
            try {
                run(query, play)
            } catch (e: Exception) {
                problem("harness failed: ${e.javaClass.simpleName}: ${e.message}")
            }
            log("done in ${SystemClock.elapsedRealtime() - started} ms, ${seenArt.size} artwork URIs checked, $problems problems")
            withContext(Dispatchers.IO) { File(filesDir, "harness.txt").writeText(out.toString()) }
            browser?.disconnect()
            finish()
        }
    }

    override fun onDestroy() {
        scope.cancel()
        super.onDestroy()
    }

    private suspend fun run(query: String?, play: Boolean) {
        val b = connect()
        val root = b.root
        val extras = b.extras ?: Bundle.EMPTY
        log("connected: root=$root")
        log("root extras: search supported=${extras.getBoolean("android.media.browse.SEARCH_SUPPORTED")}, " +
            "browsable style=${extras.getInt("android.media.browse.CONTENT_STYLE_BROWSABLE_HINT", -1)}, " +
            "playable style=${extras.getInt("android.media.browse.CONTENT_STYLE_PLAYABLE_HINT", -1)}")

        // ---- The tree ----------------------------------------------------------------------
        val tabs = children(b, root)
        log("root: ${tabs.size} tabs: ${tabs.joinToString { it.mediaId.orEmpty() }}")
        var firstAlbum: MediaBrowser.MediaItem? = null
        var firstAlbumTrack: MediaBrowser.MediaItem? = null
        for (tab in tabs) {
            val kids = children(b, tab.mediaId!!)
            log("  ${tab.mediaId} [${style(tab)}]: ${kids.size} children ${summary(kids)}")
            for (k in kids.take(if (tab.mediaId!!.endsWith("home")) 6 else 2)) {
                if (!k.isBrowsable) continue
                val grand = children(b, k.mediaId!!)
                log("    ${k.mediaId} [${kind(k)}${style(k)}]: ${grand.size} children ${summary(grand)}")
                val album = if (k.mediaId!!.contains("/album/")) k else grand.firstOrNull { it.mediaId!!.contains("/album/") }
                if (album != null && firstAlbum == null) {
                    firstAlbum = album
                    val tracks = children(b, album.mediaId!!)
                    log("      ${album.mediaId} [${kind(album)}]: ${tracks.size} tracks ${summary(tracks)}")
                    firstAlbumTrack = tracks.getOrNull(minOf(2, tracks.lastIndex))
                }
            }
        }

        // ---- Paging (Auto asks big lists a page at a time) ------------------------------------
        val shelf = "@m/shelf/recentlyadded"
        val all = children(b, shelf)
        val p0 = children(b, shelf, page = 0, size = 5)
        val p1 = children(b, shelf, page = 1, size = 5)
        val pagesOk = p0.map { it.mediaId } == all.take(5).map { it.mediaId } && p1.map { it.mediaId } == all.drop(5).take(5).map { it.mediaId }
        log("paging $shelf: ${all.size} in all, page 0 ${p0.size}, page 1 ${p1.size}: ${if (pagesOk) "matches" else "MISMATCH"}")
        if (!pagesOk) problem("paging")

        // ---- Artwork -------------------------------------------------------------------------
        checkArtwork()

        // ---- Search ----------------------------------------------------------------------------
        val q = query ?: "the"
        val results = search(b, q)
        log("search \"$q\": ${results.size} results ${summary(results)}; groups ${results.mapNotNull { it.description.extras?.getString("android.media.browse.CONTENT_STYLE_GROUP_TITLE_HINT") }.distinct()}")
        checkArtwork()

        if (!play) return
        // ---- Play, as Auto does: playFromMediaId on the session ------------------------------------
        val controller = MediaController(this, b.sessionToken)
        firstAlbum?.let { album ->
            controller.transportControls.playFromMediaId(album.mediaId, null)
            val s = waitForPlaying(controller)
            log("play album ${album.mediaId}: state ${stateName(s?.state)}, queue ${controller.queue?.size}, current ${controller.metadata?.getString("android.media.metadata.MEDIA_ID")}, position ${s?.position} ms")
            if (s?.state != PlaybackState.STATE_PLAYING) problem("album did not play")
            val actions = s?.customActions.orEmpty().map { "${it.action} (${it.name})" }
            log("custom actions in the session's playback state: $actions")
            if (actions.none { it.startsWith("marquee.FAVOURITE") }) problem("no favourite action")
            val art = controller.metadata?.getString("android.media.metadata.ART_URI") ?: controller.metadata?.getString("android.media.metadata.DISPLAY_ICON_URI")
            art?.let { seenArt += it }
            log("session metadata: art URI ${art?.let { Uri.parse(it).scheme + "://" + Uri.parse(it).authority }}, bitmap ${controller.metadata?.getBitmap("android.media.metadata.ART")?.let { "${it.width}x${it.height}" } ?: controller.metadata?.getBitmap("android.media.metadata.ALBUM_ART")?.let { "${it.width}x${it.height}" }}")
        }
        firstAlbumTrack?.let { track ->
            controller.transportControls.playFromMediaId(track.mediaId, null)
            delay(1500)
            val s = waitForPlaying(controller)
            val queue = controller.queue.orEmpty()
            val active = queue.indexOfFirst { it.queueId == s?.activeQueueItemId }
            log("play track ${track.mediaId}: state ${stateName(s?.state)}, queue ${queue.size}, at ${active} (its album, from that track)")
            if (s?.state != PlaybackState.STATE_PLAYING) problem("track did not play")
        }
        results.firstOrNull { it.isPlayable && it.mediaId!!.contains("/track/search/") }?.let { hit ->
            controller.transportControls.playFromMediaId(hit.mediaId, null)
            delay(1500)
            val s = waitForPlaying(controller)
            log("play search result ${hit.mediaId}: state ${stateName(s?.state)}, queue ${controller.queue?.size}")
        }
        controller.transportControls.pause()
        delay(500)
        log("paused: ${stateName(controller.playbackState?.state)}")
        checkArtwork()
    }

    /** Every artwork URI: the cover provider's content URI, nothing from the account in it; one is opened. */
    private suspend fun checkArtwork() {
        val creds = AppSession.ensureLoaded(applicationContext)?.credentials
        val secrets = listOfNotNull(creds?.token, creds?.salt, creds?.user, creds?.url?.let { Uri.parse(it).host }).filter { it.length >= 3 }
        var bad = 0
        for (a in seenArt) {
            val u = Uri.parse(a)
            if (u.scheme != "content" || u.authority != Covers.AUTHORITY || u.query != null) {
                bad++
                problem("artwork URI not the cover provider's: ${u.scheme}://${u.authority}")
            }
            if (secrets.any { a.contains(it, ignoreCase = true) } || a.contains("t=") || a.contains("s=")) {
                bad++
                problem("artwork URI carries a credential")
            }
        }
        log("artwork: ${seenArt.size} URIs, ${if (bad == 0) "all content://${Covers.AUTHORITY}/cover/<id>/<size>, no credentials" else "$bad bad"}")
        seenArt.firstOrNull()?.let { a ->
            val bmp = withContext(Dispatchers.IO) { runCatching { contentResolver.openInputStream(Uri.parse(a))?.use { BitmapFactory.decodeStream(it) } }.getOrNull() }
            log("opened ${Uri.parse(a).path}: ${bmp?.let { "${it.width}x${it.height}" } ?: "FAILED"}")
            if (bmp == null) problem("artwork did not open")
        }
    }

    private suspend fun connect(): MediaBrowser {
        val done = CompletableDeferred<Unit>()
        val b = MediaBrowser(
            this, ComponentName(this, PlaybackService::class.java),
            object : MediaBrowser.ConnectionCallback() {
                override fun onConnected() { done.complete(Unit) }
                override fun onConnectionFailed() { done.completeExceptionally(IllegalStateException("connection failed")) }
            },
            // Root hints as Auto sends them.
            Bundle().apply {
                putInt("androidx.media.MediaBrowserCompat.Extras.KEY_ROOT_CHILDREN_LIMIT", 4)
                putInt("android.media.extras.MEDIA_ART_SIZE_HINT_PIXELS", 300)
            },
        )
        browser = b
        b.connect()
        withTimeout(10_000) { done.await() }
        return b
    }

    private suspend fun children(b: MediaBrowser, parent: String, page: Int = -1, size: Int = -1): List<MediaBrowser.MediaItem> {
        val done = CompletableDeferred<List<MediaBrowser.MediaItem>>()
        val cb = object : MediaBrowser.SubscriptionCallback() {
            override fun onChildrenLoaded(parentId: String, children: MutableList<MediaBrowser.MediaItem>) { done.complete(children) }
            override fun onChildrenLoaded(parentId: String, children: MutableList<MediaBrowser.MediaItem>, options: Bundle) { done.complete(children) }
            override fun onError(parentId: String) { done.complete(emptyList()) }
            override fun onError(parentId: String, options: Bundle) { done.complete(emptyList()) }
        }
        if (page >= 0) b.subscribe(parent, Bundle().apply { putInt(MediaBrowser.EXTRA_PAGE, page); putInt(MediaBrowser.EXTRA_PAGE_SIZE, size) }, cb)
        else b.subscribe(parent, cb)
        val list = withTimeout(30_000) { done.await() }
        b.unsubscribe(parent, cb)
        list.forEach { it.description.iconUri?.let { u -> seenArt += u.toString() } }
        return list
    }

    /**
     * Search as Auto sends it: the platform browser has no search, so this goes through the compat
     * browser (Media3 ships the legacy MediaBrowserCompat), which is what Auto's own client uses.
     */
    @SuppressLint("RestrictedApi")
    private suspend fun search(@Suppress("UNUSED_PARAMETER") b: MediaBrowser, query: String): List<MediaBrowser.MediaItem> {
        val connected = CompletableDeferred<Unit>()
        val compat = MediaBrowserCompat(
            this, ComponentName(this, PlaybackService::class.java),
            object : MediaBrowserCompat.ConnectionCallback() {
                override fun onConnected() { connected.complete(Unit) }
                override fun onConnectionFailed() { connected.completeExceptionally(IllegalStateException("compat connection failed")) }
            },
            null,
        )
        compat.connect()
        try {
            withTimeout(10_000) { connected.await() }
            val done = CompletableDeferred<List<MediaBrowser.MediaItem>>()
            compat.search(query, null, object : MediaBrowserCompat.SearchCallback() {
                override fun onSearchResult(query: String, extras: Bundle?, items: List<MediaBrowserCompat.MediaItem>) {
                    done.complete(items.map { MediaBrowser.MediaItem.CREATOR.createFromParcel(parcel(it)) })
                }
                override fun onError(query: String, extras: Bundle?) { done.complete(emptyList()) }
            })
            val list = withTimeout(30_000) { done.await() }
            list.forEach { it.description.iconUri?.let { u -> seenArt += u.toString() } }
            return list
        } finally {
            compat.disconnect()
        }
    }

    /** A compat item as the platform's (the same parcel layout), so the rest of the harness reads one kind. */
    @SuppressLint("RestrictedApi")
    private fun parcel(item: MediaBrowserCompat.MediaItem): android.os.Parcel {
        val p = android.os.Parcel.obtain()
        item.writeToParcel(p, 0)
        p.setDataPosition(0)
        return p
    }

    private suspend fun waitForPlaying(c: MediaController): PlaybackState? {
        repeat(40) {
            val s = c.playbackState
            if (s?.state == PlaybackState.STATE_PLAYING) return s
            delay(250)
        }
        return c.playbackState
    }

    private fun kind(i: MediaBrowser.MediaItem) = when {
        i.isBrowsable && i.isPlayable -> "browsable+playable"
        i.isBrowsable -> "browsable"
        i.isPlayable -> "playable"
        else -> "?"
    }

    private fun style(i: MediaBrowser.MediaItem): String {
        val e = i.description.extras ?: return ""
        val b = e.getInt("android.media.browse.CONTENT_STYLE_BROWSABLE_HINT", -1)
        return if (b < 0) "" else " browsable style $b"
    }

    private fun summary(items: List<MediaBrowser.MediaItem>): String {
        if (items.isEmpty()) return ""
        val kinds = items.groupingBy { kind(it) }.eachCount()
        val art = items.count { it.description.iconUri != null }
        return "($kinds, $art with artwork, e.g. ${items.first().mediaId})"
    }

    private fun stateName(s: Int?) = when (s) {
        PlaybackState.STATE_PLAYING -> "PLAYING"
        PlaybackState.STATE_PAUSED -> "PAUSED"
        PlaybackState.STATE_BUFFERING -> "BUFFERING"
        PlaybackState.STATE_STOPPED -> "STOPPED"
        PlaybackState.STATE_NONE -> "NONE"
        PlaybackState.STATE_ERROR -> "ERROR"
        else -> "$s"
    }

    private companion object {
        const val TAG = "MarqueeHarness"
        @Suppress("unused") val SERVICE = MediaBrowserService.SERVICE_INTERFACE
    }
}
