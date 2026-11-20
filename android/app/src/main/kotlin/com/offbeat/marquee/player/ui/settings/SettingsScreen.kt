package com.offbeat.marquee.player.ui.settings

import com.offbeat.marquee.player.diag.AppLog
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalUriHandler
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.offbeat.marquee.core.playback.ReplayGainMode
import com.offbeat.marquee.core.playback.StreamQuality
import com.offbeat.marquee.core.settings.AccentChoice
import com.offbeat.marquee.core.settings.CacheCap
import com.offbeat.marquee.core.settings.PrefetchAhead
import com.offbeat.marquee.core.settings.QualityChoices
import com.offbeat.marquee.core.diag.accountSummary
import com.offbeat.marquee.core.subsonic.ServerInfo
import com.offbeat.marquee.core.subsonic.SubsonicError
import com.offbeat.marquee.player.BuildConfig
import com.offbeat.marquee.player.data.Library
import com.offbeat.marquee.player.data.LyricsRepo
import com.offbeat.marquee.player.data.OnlineLyrics
import com.offbeat.marquee.player.diag.Diagnostics
import com.offbeat.marquee.player.eq.EqStore
import com.offbeat.marquee.player.net.NetworkMonitor
import com.offbeat.marquee.player.offline.Availability
import com.offbeat.marquee.player.offline.Downloads
import com.offbeat.marquee.player.playback.MediaStores
import com.offbeat.marquee.player.playback.PlaybackSettings
import com.offbeat.marquee.player.session.AppSession
import com.offbeat.marquee.player.session.SessionState
import com.offbeat.marquee.player.settings.UiSettings
import com.offbeat.marquee.player.ui.components.BackButton
import com.offbeat.marquee.player.ui.components.LocalScreenTop
import com.offbeat.marquee.player.ui.components.formatBytes
import com.offbeat.marquee.player.ui.theme.Palette
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import com.offbeat.marquee.player.ui.components.rememberKeptScrollState

private const val TAG = "MarqueeSettings"

/** The server as Settings names it: "Navidrome 0.64.2", or the API version for another server. */
fun describeServer(info: ServerInfo): String {
    val type = info.type?.takeIf { it.isNotBlank() }?.replaceFirstChar { it.uppercase() }
    // Navidrome says "0.64.2 (10114574)": the build in brackets is left out.
    val version = info.serverVersion?.substringBefore(" (")?.trim()
    return when {
        type != null && !version.isNullOrBlank() -> "$type $version"
        type != null -> type
        else -> "Subsonic API ${info.version}"
    }
}

/**
 * Settings: one short list of sections (docs/DESIGN.md, "Screens"), each a card of rows. The server's name
 * and version come from a `ping` when the page opens online. Only the release build shows the
 * account (user and address); debug and benchmark builds never do.
 */
@Composable
fun SettingsScreen(
    session: SessionState.SignedIn,
    onBack: () -> Unit,
    onSignOut: () -> Unit,
    onOpenDownloads: () -> Unit,
    onOpenEqualizer: () -> Unit,
    onOpenShelves: () -> Unit,
    onOpenLicences: () -> Unit,
) {
    val server by UiSettings.server.collectAsStateWithLifecycle()
    LaunchedEffect(Unit) {
        if (!NetworkMonitor.online) return@LaunchedEffect
        val c = AppSession.current ?: return@LaunchedEffect
        try {
            UiSettings.setServer(describeServer(c.ping()))
        } catch (e: SubsonicError) {
            AppLog.i(TAG, "ping for the server version failed (${e::class.simpleName})")
        }
    }
    Box(Modifier.fillMaxSize(), contentAlignment = Alignment.TopCenter) {
        Column(
            Modifier
                .widthIn(max = SettingsMaxWidth)
                .fillMaxSize()
                .verticalScroll(rememberKeptScrollState())
                .padding(top = LocalScreenTop.current + 4.dp, bottom = 32.dp),
        ) {
            Row(Modifier.padding(start = 8.dp, bottom = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                BackButton(onBack)
                Text(
                    "Settings",
                    style = MaterialTheme.typography.headlineSmall,
                    modifier = Modifier.padding(start = 12.dp).semantics { heading() },
                )
            }
            Column(Modifier.padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(SectionGap)) {
                AccountSection(session, server, onSignOut)
                StreamingSection()
                DownloadsSection(onOpenDownloads)
                CacheSection()
                PlaybackSection(session, onOpenEqualizer)
                AppearanceSection()
                HomeSection(onOpenShelves)
                LyricsSection()
                AboutSection(session, server, onOpenLicences)
            }
        }
    }
}

@Composable
private fun AccountSection(session: SessionState.SignedIn, server: String?, onSignOut: () -> Unit) {
    var confirm by rememberSaveable { mutableStateOf(false) }
    val stored by Downloads.stored.collectAsStateWithLifecycle()
    SettingCard("Account") {
        // The release build names the account (the user and the server's address); debug and
        // benchmark builds, the test builds that get run and screenshotted, never do: READ_ONLY is a
        // build-time constant, so R8 drops this branch from them.
        if (!BuildConfig.READ_ONLY) {
            ReleaseAccountRow(server)
        } else {
            SettingRow(
                name = server?.let { "Signed in to $it" } ?: "Signed in",
                hint = if (session.readOnly) "Read-only: nothing is written to the server" else "Favourites, ratings and plays go to your server",
            )
        }
        RowDivider()
        SettingRow(
            name = "Sign out",
            hint = "Removes downloads and caches from this phone",
            trailing = {
                OutlinedButton(onClick = { confirm = true }, modifier = Modifier.heightIn(min = 48.dp)) { Text("Sign out") }
            },
        )
    }
    if (confirm) {
        val downloads = if (stored.complete.isEmpty()) "" else " (${stored.complete.size} songs, ${formatBytes(stored.totalBytes)})"
        AlertDialog(
            onDismissRequest = { confirm = false },
            title = { Text("Sign out?") },
            text = { Text("Your downloads$downloads, the cache, the saved covers and lyrics are removed from this phone. Signing in again downloads nothing back by itself.") },
            confirmButton = {
                TextButton(onClick = { confirm = false; onSignOut() }) { Text("Sign out", color = Palette.current.accent) }
            },
            dismissButton = { TextButton(onClick = { confirm = false }) { Text("Cancel", color = Palette.current.text) } },
            containerColor = Palette.current.surfaceHi,
        )
    }
}

/** Release builds only: "Signed in as dana", then `music.example.com · Navidrome 0.64.2`. Never the token or salt. */
@Composable
private fun ReleaseAccountRow(server: String?) {
    val client by AppSession.client.collectAsStateWithLifecycle()
    val creds = client?.credentials
    if (creds == null) {
        SettingRow(name = server?.let { "Signed in to $it" } ?: "Signed in", hint = "Favourites, ratings and plays go to your server")
        return
    }
    SettingRow(name = "Signed in as ${creds.user}", hint = accountSummary(creds.url, server))
}

/** The hint, and for an Opus choice which files are left as they are ([StreamQuality.forSource]). */
private fun opusNote(hint: String, q: StreamQuality, verb: String): String =
    q.opusUnchangedUpTo?.let { "$hint. Opus files up to $it kbps $verb unchanged" } ?: hint

@Composable
private fun StreamingSection() {
    val quality by PlaybackSettings.qualityFlow.collectAsStateWithLifecycle()
    SettingCard("Streaming") {
        SegmentedSettingRow(
            name = "On Wi-Fi",
            hint = opusNote("Original, or Opus at this many kbps", quality.unmetered, "play"),
            options = QualityChoices.OFFERED,
            selected = QualityChoices.nearestOffered(quality.unmetered),
            label = QualityChoices::shortLabel,
            describe = QualityChoices::label,
            onSelect = { PlaybackSettings.quality = quality.copy(unmetered = it) },
        )
        RowDivider()
        SegmentedSettingRow(
            name = "On mobile data",
            hint = opusNote("Opus 128 sounds close to the original at a tenth of the data", quality.metered, "play"),
            options = QualityChoices.OFFERED,
            selected = QualityChoices.nearestOffered(quality.metered),
            label = QualityChoices::shortLabel,
            describe = QualityChoices::label,
            onSelect = { PlaybackSettings.quality = quality.copy(metered = it) },
        )
    }
}

@Composable
private fun DownloadsSection(onOpenDownloads: () -> Unit) {
    val stored by Downloads.stored.collectAsStateWithLifecycle()
    val quality by Downloads.quality.collectAsStateWithLifecycle()
    val wifiOnly by Downloads.wifiOnly.collectAsStateWithLifecycle()
    SettingCard("Downloads") {
        SegmentedSettingRow(
            name = "Quality",
            hint = opusNote("For new downloads; what is downloaded stays as it is", quality, "are downloaded"),
            options = Downloads.QUALITIES,
            selected = QualityChoices.nearestOffered(quality),
            label = QualityChoices::shortLabel,
            describe = QualityChoices::label,
            onSelect = Downloads::setQuality,
        )
        RowDivider()
        SwitchSettingRow(
            name = "Wi-Fi only",
            hint = "Downloads wait for Wi-Fi and carry on when it comes back",
            checked = wifiOnly,
            onToggle = Downloads::setWifiOnly,
        )
        RowDivider()
        InfoRow("Space used", "${formatBytes(stored.totalBytes)} · ${stored.complete.size} songs")
        RowDivider()
        NavSettingRow("Manage downloads", "What is downloaded, and Remove", onOpenDownloads)
    }
}

@Composable
private fun CacheSection() {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val cap by PlaybackSettings.cacheCapFlow.collectAsStateWithLifecycle()
    val prefetch by PlaybackSettings.prefetchFlow.collectAsStateWithLifecycle()
    var used by remember { mutableStateOf<Long?>(null) }
    LaunchedEffect(cap) { used = withContext(Dispatchers.IO) { MediaStores.cache(context).cacheSpace } }
    SettingCard("Cache") {
        SegmentedSettingRow(
            name = "Size",
            hint = "Recent and upcoming tracks, never your downloads. Lowering it frees space now",
            options = CacheCap.CHOICES_GB,
            selected = CacheCap.gbOf(cap),
            label = { "$it GB" },
            onSelect = { gb ->
                val bytes = CacheCap.bytes(gb)
                PlaybackSettings.cacheCapBytes = bytes
                scope.launch {
                    withContext(Dispatchers.IO) {
                        MediaStores.setCap(context, bytes)
                        Availability.refreshCached()
                    }
                    used = withContext(Dispatchers.IO) { MediaStores.cache(context).cacheSpace }
                }
            },
        )
        RowDivider()
        SettingRow(
            name = "Space used",
            hint = used?.let { "${formatBytes(it)} of ${CacheCap.gbOf(cap)} GB" } ?: "…",
            trailing = {
                OutlinedButton(
                    onClick = {
                        scope.launch {
                            val freed = withContext(Dispatchers.IO) { MediaStores.clearCache(context) }
                            withContext(Dispatchers.IO) { Availability.refreshCached() }
                            used = withContext(Dispatchers.IO) { MediaStores.cache(context).cacheSpace }
                            Library.post("Cache cleared: ${formatBytes(freed)} freed")
                        }
                    },
                    modifier = Modifier.heightIn(min = 48.dp),
                ) { Text("Clear") }
            },
        )
        RowDivider()
        SegmentedSettingRow(
            name = "Prefetch ahead",
            hint = "Tracks fetched whole before they play, so tunnels don't stop the music",
            options = PrefetchAhead.RANGE.toList(),
            selected = prefetch,
            label = { "$it" },
            describe = { if (it == 1) "1 track" else "$it tracks" },
            onSelect = { PlaybackSettings.prefetchCount = it },
        )
    }
}

@Composable
private fun PlaybackSection(session: SessionState.SignedIn, onOpenEqualizer: () -> Unit) {
    val palette = Palette.current
    val replayGain by PlaybackSettings.replayGainFlow.collectAsStateWithLifecycle()
    val scrobbling by PlaybackSettings.scrobblingFlow.collectAsStateWithLifecycle()
    val eq by EqStore.setting.collectAsStateWithLifecycle()
    SettingCard("Playback") {
        SegmentedSettingRow(
            name = "ReplayGain",
            hint = "Evens out loudness: per track, or per album when it plays in order",
            options = ReplayGainMode.entries,
            selected = replayGain,
            label = { it.name },
            onSelect = { PlaybackSettings.replayGain = it },
        )
        RowDivider()
        NavSettingRow(
            name = "Equalizer",
            hint = if (eq.enabled) eq.label else "Off",
            hintColor = if (eq.enabled) palette.accent else palette.textDim,
            onClick = onOpenEqualizer,
        )
        RowDivider()
        SwitchSettingRow(
            name = "Scrobbling",
            hint = if (BuildConfig.READ_ONLY && session.readOnly) "Read-only test build: scrobbles are refused and never sent" else "Tells your server what you played (its play counts and Last.fm)",
            checked = scrobbling,
            onToggle = { PlaybackSettings.scrobbling = it },
        )
    }
}

@Composable
private fun AppearanceSection() {
    val accent by UiSettings.accent.collectAsStateWithLifecycle()
    val pureBlack by UiSettings.pureBlack.collectAsStateWithLifecycle()
    // The last fixed colour, kept while "From the cover" is chosen so switching back finds it.
    var lastFixed by rememberSaveable { mutableStateOf((accent as? AccentChoice.Fixed)?.rgb ?: AccentChoice.SWATCHES.first()) }
    SettingCard("Appearance") {
        SettingRow(
            name = "Accent",
            hint = if (accent is AccentChoice.Fixed) "One colour, whatever plays" else "Taken from the playing cover, as on the desktop",
            below = {
                Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                    Segmented(
                        name = "Accent",
                        options = listOf(false, true),
                        selected = accent is AccentChoice.Fixed,
                        label = { if (it) "Fixed" else "From the cover" },
                        onSelect = { fixed -> UiSettings.setAccent(if (fixed) AccentChoice.Fixed(lastFixed) else AccentChoice.FromCover) },
                    )
                    val fixed = accent as? AccentChoice.Fixed
                    if (fixed != null) {
                        SwatchPicker(
                            colors = AccentChoice.SWATCHES.map { Color(0xFF000000 or it.toLong()) },
                            names = AccentChoice.NAMES,
                            selected = AccentChoice.SWATCHES.indexOf(fixed.rgb),
                            onSelect = { i ->
                                lastFixed = AccentChoice.SWATCHES[i]
                                UiSettings.setAccent(AccentChoice.Fixed(lastFixed))
                            },
                        )
                    }
                }
            },
        )
        RowDivider()
        SwitchSettingRow(
            name = "Pure black",
            hint = "Black background for OLED screens",
            checked = pureBlack,
            onToggle = UiSettings::setPureBlack,
        )
    }
}

@Composable
private fun HomeSection(onOpenShelves: () -> Unit) {
    val layout by UiSettings.shelves.collectAsStateWithLifecycle()
    val shown = layout.visible.size
    val total = layout.order.size
    SettingCard("Home") {
        NavSettingRow(
            name = "Edit shelves",
            hint = when {
                layout.isDefault -> "All $total shelves, in the usual order"
                shown == total -> "All $total shelves, reordered"
                else -> "$shown of $total shelves shown"
            },
            onClick = onOpenShelves,
        )
    }
}

@Composable
private fun LyricsSection() {
    val lyricsOnline by UiSettings.lyricsOnline.collectAsStateWithLifecycle()
    // Test builds have no LRCLIB: the switch is kept (and stored) but nothing is ever sent.
    val inThisBuild = LyricsRepo.online() != OnlineLyrics.OffInThisBuild
    SettingCard("Lyrics") {
        SwitchSettingRow(
            name = "Look up online",
            hint = when {
                !inThisBuild -> "Off in test builds: LRCLIB is never contacted"
                LyricsRepo.usesTestStandIn -> "Test build: asks a local stand-in for LRCLIB, never lrclib.net"
                else -> "When your server has none, asks LRCLIB (lrclib.net) by artist, title, album and length"
            },
            checked = lyricsOnline && inThisBuild,
            enabled = inThisBuild,
            onToggle = UiSettings::setLyricsOnline,
        )
    }
}

@Composable
private fun AboutSection(session: SessionState.SignedIn, server: String?, onOpenLicences: () -> Unit) {
    SettingCard("About") {
        InfoRow("Marquee Player", "${BuildConfig.VERSION_NAME.removeSuffix("-" + BuildConfig.BUILD_TYPE)} · ${BuildConfig.BUILD_TYPE} build")
        RowDivider()
        InfoRow("Server", server ?: "Not known yet")
        if (BuildConfig.READ_ONLY && session.readOnly) {
            RowDivider()
            SettingRow(
                name = "Read-only test build",
                hint = if (session.testServer) "Signed in to the test server; nothing is ever written to it" else "Nothing is ever written to the server",
                hintColor = Palette.current.textDim,
            )
        }
        RowDivider()
        ShareDiagnosticsRow()
        RowDivider()
        SettingRow(
            name = "Licence",
            hint = "© 2026 Offbeat · Free software under the GNU General Public License, version 3 or later (GPL-3.0-or-later), with no warranty",
        )
        if (BuildConfig.SOURCE_URL.isNotEmpty()) {
            val uriHandler = LocalUriHandler.current
            RowDivider()
            NavSettingRow("Source code", BuildConfig.SOURCE_URL, { runCatching { uriHandler.openUri(BuildConfig.SOURCE_URL) } })
        }
        RowDivider()
        NavSettingRow("Open-source licences", "The app's licence in full, and the libraries it is built with", onOpenLicences)
    }
}

/** Builds the diagnostics file and opens the share sheet; nothing leaves the phone otherwise. */
@Composable
private fun ShareDiagnosticsRow() {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var busy by remember { mutableStateOf(false) }
    SettingRow(
        name = "Share diagnostics",
        hint = "The app and phone, your settings, what's stored, the last crash and the app's log, " +
            "without your sign-in or server address. Nothing is sent unless you share it.",
        trailing = {
            OutlinedButton(
                onClick = {
                    if (busy) return@OutlinedButton
                    busy = true
                    scope.launch {
                        try {
                            val intent = withContext(Dispatchers.IO) { Diagnostics.shareIntent(context) }
                            context.startActivity(intent)
                        } catch (e: Exception) {
                            AppLog.w(TAG, "diagnostics not shared (${e.javaClass.simpleName})")
                            Library.post("Couldn't build the diagnostics file")
                        } finally {
                            busy = false
                        }
                    }
                },
                enabled = !busy,
                modifier = Modifier.heightIn(min = 48.dp),
            ) { Text("Share") }
        },
    )
}
