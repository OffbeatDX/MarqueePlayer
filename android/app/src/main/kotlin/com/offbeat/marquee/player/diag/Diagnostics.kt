package com.offbeat.marquee.player.diag

import android.content.ClipData
import android.content.Context
import android.content.Intent
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import android.os.Build
import androidx.core.content.FileProvider
import com.offbeat.marquee.core.diag.DiagnosticReport
import com.offbeat.marquee.core.diag.LogLine
import com.offbeat.marquee.core.settings.AccentChoice
import com.offbeat.marquee.core.settings.CacheCap
import com.offbeat.marquee.core.settings.QualityChoices
import com.offbeat.marquee.player.BuildConfig
import com.offbeat.marquee.player.eq.EqStore
import com.offbeat.marquee.player.net.NetworkMonitor
import com.offbeat.marquee.player.offline.Downloads
import com.offbeat.marquee.player.playback.MediaStores
import com.offbeat.marquee.player.playback.PlaybackSettings
import com.offbeat.marquee.player.settings.LibraryPrefs
import com.offbeat.marquee.player.settings.UiSettings
import com.offbeat.marquee.player.ui.components.formatBytes
import java.io.File
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale

/**
 * "Share diagnostics": one text file the user can send to whoever helps when something goes wrong. The
 * app and the phone, the server's version, the network at the time, every setting (never the
 * account), what is stored, the last crash and the log, scrubbed as a whole once more. It is
 * written to `cache/diagnostics` and handed to the share sheet; nothing is sent unless the
 * person shares it.
 */
object Diagnostics {
    private const val DIR = "diagnostics"

    /** Builds the report (blocking: call it off the main thread) and returns the share intent. */
    fun shareIntent(context: Context): Intent {
        val file = write(context)
        val uri = FileProvider.getUriForFile(context, "${context.packageName}.diagnostics", file)
        val send = Intent(Intent.ACTION_SEND).apply {
            type = "text/plain"
            putExtra(Intent.EXTRA_STREAM, uri)
            putExtra(Intent.EXTRA_SUBJECT, "Marquee Player diagnostics")
            clipData = ClipData.newRawUri(file.name, uri)
            addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
        }
        return Intent.createChooser(send, "Share diagnostics")
    }

    /** The report file, replacing any earlier one. */
    fun write(context: Context): File {
        val dir = File(context.cacheDir, DIR).apply { mkdirs() }
        dir.listFiles()?.forEach { it.delete() }
        val stamp = SimpleDateFormat("yyyyMMdd-HHmmss", Locale.ROOT).format(Date())
        val file = File(dir, "marquee-diagnostics-$stamp.txt")
        file.writeText(build(context))
        AppLog.i("MarqueeDiag", "diagnostics report built (${file.length()} bytes)")
        return file
    }

    fun build(context: Context): String {
        val log = AppLog.read()
        return DiagnosticReport("Marquee Player diagnostics, ${LogLine.time(System.currentTimeMillis())}")
            .section("App", app())
            .section("Device", device(context))
            .section("Server and network", serverAndNetwork(context))
            .section("Settings", settings())
            .section("Storage", storage(context))
            .text("Last crash", AppLog.lastCrash())
            .text("Log (oldest first)", log)
            .render(AppLog.currentScrubber)
    }

    private fun app() = listOf(
        "Version" to "${BuildConfig.VERSION_NAME} (${BuildConfig.VERSION_CODE})",
        "Build type" to BuildConfig.BUILD_TYPE + if (BuildConfig.READ_ONLY) ", read-only" else "",
        "Package" to BuildConfig.APPLICATION_ID,
    )

    private fun device(context: Context): List<Pair<String, String>> {
        val m = context.resources.displayMetrics
        val c = context.resources.configuration
        return listOf(
            "Maker" to Build.MANUFACTURER,
            "Model" to "${Build.MODEL} (${Build.DEVICE})",
            "Android" to "${Build.VERSION.RELEASE} (SDK ${Build.VERSION.SDK_INT})",
            "Screen" to "${m.widthPixels} x ${m.heightPixels} px, ${c.screenWidthDp} x ${c.screenHeightDp} dp, ${m.densityDpi} dpi (x${"%.2f".format(Locale.ROOT, m.density)})",
            "Font scale" to "%.2f".format(Locale.ROOT, c.fontScale),
            "Language" to Locale.getDefault().toLanguageTag(),
        )
    }

    private fun serverAndNetwork(context: Context): List<Pair<String, String>> {
        val cm = context.getSystemService(ConnectivityManager::class.java)
        val caps = cm?.activeNetwork?.let(cm::getNetworkCapabilities)
        val type = when {
            caps == null -> "none"
            else -> buildList {
                if (caps.hasTransport(NetworkCapabilities.TRANSPORT_WIFI)) add("Wi-Fi")
                if (caps.hasTransport(NetworkCapabilities.TRANSPORT_CELLULAR)) add("mobile data")
                if (caps.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET)) add("Ethernet")
                if (caps.hasTransport(NetworkCapabilities.TRANSPORT_BLUETOOTH)) add("Bluetooth")
                if (caps.hasTransport(NetworkCapabilities.TRANSPORT_VPN)) add("VPN")
                if (isEmpty()) add("other")
            }.joinToString(" + ")
        }
        val flags = caps?.let {
            listOfNotNull(
                if (it.hasCapability(NetworkCapabilities.NET_CAPABILITY_NOT_METERED)) "unmetered" else "metered",
                if (it.hasCapability(NetworkCapabilities.NET_CAPABILITY_VALIDATED)) "validated" else "not validated",
                "down ${it.linkDownstreamBandwidthKbps / 1000} Mbps",
            ).joinToString(", ")
        }
        val app = NetworkMonitor.state.value
        return listOf(
            "Server" to (UiSettings.server.value ?: "not known yet"),
            "Network" to (if (flags != null) "$type ($flags)" else type),
            "The app sees" to if (!app.online) "offline" else if (app.unmetered) "online, unmetered" else "online, metered",
        )
    }

    private fun settings(): List<Pair<String, String>> {
        val q = PlaybackSettings.qualityFlow.value
        val eq = EqStore.setting.value
        val accent = when (val a = UiSettings.accent.value) {
            AccentChoice.FromCover -> "from the cover"
            is AccentChoice.Fixed -> AccentChoice.NAMES.getOrNull(AccentChoice.SWATCHES.indexOf(a.rgb)) ?: "fixed #%06X".format(a.rgb and 0xFFFFFF)
        }
        val shelves = UiSettings.shelves.value
        return listOf(
            "Streaming on Wi-Fi" to QualityChoices.label(q.unmetered),
            "Streaming on mobile data" to QualityChoices.label(q.metered),
            "Download quality" to QualityChoices.label(Downloads.quality.value),
            "Downloads Wi-Fi only" to onOff(Downloads.wifiOnly.value),
            "Cache size" to "${CacheCap.gbOf(PlaybackSettings.cacheCapFlow.value)} GB",
            "Prefetch ahead" to "${PlaybackSettings.prefetchFlow.value} tracks",
            "ReplayGain" to PlaybackSettings.replayGainFlow.value.name,
            "Equalizer" to if (eq.enabled) "on, ${eq.mode.name.lowercase()}, ${eq.label}" else "off",
            "Audio offload" to onOff(PlaybackSettings.audioOffload) + if (eq.enabled) " (not used while the equalizer is on)" else "",
            "Scrobbling" to onOff(PlaybackSettings.scrobblingFlow.value),
            "Accent" to accent,
            "Pure black" to onOff(UiSettings.pureBlack.value),
            "Home shelves" to if (shelves.isDefault) "default" else shelves.encode(),
            "Lyrics: look up online" to onOff(UiSettings.lyricsOnline.value),
            "Library sort" to LibraryPrefs.sort.value.title,
        )
    }

    private fun storage(context: Context): List<Pair<String, String>> {
        val stored = Downloads.stored.value
        val pins = Downloads.pins.value.pins
        val missing = runCatching { Downloads.missing().size }.getOrDefault(-1)
        val cache = runCatching { MediaStores.cache(context).cacheSpace }.getOrDefault(-1)
        val rows = mutableListOf(
            "Downloads" to "${formatBytes(stored.totalBytes)}, ${stored.complete.size} songs, ${pins.size} albums/playlists" +
                (if (missing > 0) ", $missing songs still to fetch" else "") +
                (if (stored.failed.isNotEmpty()) ", ${stored.failed.size} refused by the server" else ""),
            "Track cache" to "${if (cache >= 0) formatBytes(cache) else "?"} of ${CacheCap.gbOf(PlaybackSettings.cacheCapFlow.value)} GB",
            "Log" to formatBytes(AppLog.sizeBytes()),
            "Free on the phone" to formatBytes(context.filesDir.usableSpace),
        )
        // Every folder the app keeps, for anything the rows above don't cover.
        for ((root, label) in listOf(context.filesDir to "files", context.cacheDir to "cache")) {
            root.listFiles()?.filter { it.isDirectory }?.sortedBy { it.name }?.forEach { d ->
                rows += "  $label/${d.name}" to formatBytes(sizeOf(d))
            }
        }
        return rows
    }

    private fun sizeOf(f: File): Long = if (f.isFile) f.length() else f.listFiles()?.sumOf(::sizeOf) ?: 0L

    private fun onOff(b: Boolean) = if (b) "on" else "off"
}
