package com.offbeat.marquee.player.ui.downloads

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.Close
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.offbeat.marquee.core.offline.Pin
import com.offbeat.marquee.core.offline.PinKind
import com.offbeat.marquee.player.art.CoverImage
import com.offbeat.marquee.player.offline.Downloads
import com.offbeat.marquee.player.offline.PinProgress
import com.offbeat.marquee.player.ui.components.BackButton
import com.offbeat.marquee.player.ui.components.LocalScreenTop
import com.offbeat.marquee.player.ui.components.pinProgress
import com.offbeat.marquee.player.ui.theme.Palette
import com.offbeat.marquee.player.ui.theme.Tokens
import kotlinx.coroutines.launch
import com.offbeat.marquee.player.ui.components.formatBytes
import com.offbeat.marquee.player.ui.actions.songCountLabel

/**
 * Everything pinned for offline: each item's state and size, the space used in all, Remove per
 * item and Remove all. Albums and playlists open their pages (which work offline).
 */
@Composable
fun DownloadsScreen(onBack: () -> Unit, onOpenAlbum: (String) -> Unit, onOpenPlaylist: (String) -> Unit) {
    val palette = Palette.current
    val pins by Downloads.pins.collectAsStateWithLifecycle()
    val stored by Downloads.stored.collectAsStateWithLifecycle()
    val wifiOnly by Downloads.wifiOnly.collectAsStateWithLifecycle()
    val quality by Downloads.quality.collectAsStateWithLifecycle()
    val scope = rememberCoroutineScope()
    var confirmAll by remember { mutableStateOf(false) }
    val list = remember(pins) { pins.pins.sortedByDescending { it.pinnedAtMs } }
    LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(top = LocalScreenTop.current + 4.dp, bottom = 24.dp)) {
        item(key = "title") {
            Row(Modifier.padding(start = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                BackButton(onBack)
                Text("Downloads", style = MaterialTheme.typography.headlineSmall, modifier = Modifier.padding(start = 12.dp).semantics { heading() })
            }
        }
        item(key = "summary") {
            Column(Modifier.padding(horizontal = Tokens.PageMargin, vertical = 12.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                Text("${formatBytes(stored.totalBytes)} used", style = MaterialTheme.typography.titleMedium)
                val songs = pins.wanted().size
                Text(
                    "${stored.complete.size} of $songs songs downloaded · ${qualityName(quality.id)}" + if (wifiOnly) " · Wi-Fi only" else "",
                    style = MaterialTheme.typography.bodySmall,
                    color = palette.textDim,
                )
                if (list.isNotEmpty()) {
                    OutlinedButton(onClick = { confirmAll = true }, modifier = Modifier.padding(top = 8.dp).heightIn(min = 48.dp)) { Text("Remove all") }
                }
            }
        }
        if (list.isEmpty()) {
            item(key = "empty") {
                Text(
                    "Nothing downloaded. Use Download on an album or playlist page, or in the long-press menu.",
                    color = palette.textDim,
                    modifier = Modifier.padding(Tokens.PageMargin),
                )
            }
        }
        items(list, key = { it.key }) { pin ->
            PinRow(
                pin,
                onOpen = {
                    when (pin.kind) {
                        PinKind.Album -> onOpenAlbum(pin.id)
                        PinKind.Playlist -> onOpenPlaylist(pin.id)
                        PinKind.Song -> pin.songs.firstOrNull()?.albumId?.let(onOpenAlbum)
                    }
                },
                onRemove = { scope.launch { Downloads.unpin(pin.key) } },
            )
        }
    }
    if (confirmAll) {
        AlertDialog(
            onDismissRequest = { confirmAll = false },
            title = { Text("Remove every download?") },
            text = { Text("${formatBytes(stored.totalBytes)} will be freed. Everything will need the network again.") },
            confirmButton = { TextButton(onClick = { confirmAll = false; scope.launch { Downloads.removeAll() } }) { Text("Remove all") } },
            dismissButton = { TextButton(onClick = { confirmAll = false }) { Text("Keep") } },
            containerColor = palette.surface,
        )
    }
}

fun qualityName(id: String): String = when (id) {
    "original" -> "Original quality"
    else -> id.removePrefix("opus").let { "Opus $it kbps" }
}

@Composable
private fun PinRow(pin: Pin, onOpen: () -> Unit, onRemove: () -> Unit) {
    val palette = Palette.current
    val p = pinProgress(pin) ?: return
    val kind = when (pin.kind) {
        PinKind.Album -> "Album"
        PinKind.Playlist -> "Playlist"
        PinKind.Song -> "Song"
    }
    Row(
        Modifier
            .fillMaxWidth()
            .heightIn(min = 72.dp)
            .clickable(role = Role.Button, onClickLabel = "Open", onClick = onOpen)
            .padding(start = Tokens.PageMargin, end = 4.dp, top = 6.dp, bottom = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(14.dp),
    ) {
        CoverImage(pin.coverArt, Modifier.size(52.dp), shape = RoundedCornerShape(Tokens.RadiusCover))
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Text(pin.name, style = MaterialTheme.typography.bodyLarge, maxLines = 1, overflow = TextOverflow.Ellipsis)
            val songs = songCountLabel(p.total)
            Text(
                listOfNotNull(kind, pin.artist?.takeIf { pin.kind != PinKind.Playlist }, songs, formatBytes(p.bytes)).joinToString(" · "),
                style = MaterialTheme.typography.bodySmall,
                color = palette.textDim,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            Text(
                p.label,
                style = MaterialTheme.typography.labelMedium,
                color = if (p.complete) palette.textDim else palette.accent,
            )
            if (!p.complete) {
                LinearProgressIndicator(
                    progress = { p.fraction },
                    modifier = Modifier.fillMaxWidth().padding(top = 4.dp),
                    color = if (p.state == PinProgress.State.Downloading) palette.accent else palette.textFaint,
                    trackColor = palette.surfaceHi,
                )
            }
        }
        IconButton(onClick = onRemove, modifier = Modifier.size(48.dp)) {
            Icon(Icons.Rounded.Close, contentDescription = "Remove the download of ${pin.name}", tint = palette.textDim)
        }
    }
}
