package com.offbeat.marquee.player.ui.components

import com.offbeat.marquee.player.AppScope
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.size
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.ArrowDownward
import androidx.compose.material.icons.rounded.CloudOff
import androidx.compose.material.icons.rounded.DownloadDone
import androidx.compose.material.icons.rounded.DownloadForOffline
import androidx.compose.material.icons.rounded.Wifi
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.offbeat.marquee.core.offline.Pin
import com.offbeat.marquee.core.offline.PinKind
import com.offbeat.marquee.player.data.Item
import com.offbeat.marquee.player.net.NetworkMonitor
import com.offbeat.marquee.player.offline.Downloads
import com.offbeat.marquee.player.offline.PinProgress
import com.offbeat.marquee.player.ui.theme.Palette
import kotlinx.coroutines.launch

/** A pin's progress, recomposed as the worker moves on (no polling: the flows say when). */
@Composable
fun pinProgress(pin: Pin?): PinProgress? {
    val stored by Downloads.stored.collectAsStateWithLifecycle()
    val activity by Downloads.activity.collectAsStateWithLifecycle()
    val net by NetworkMonitor.state.collectAsStateWithLifecycle()
    val wifiOnly by Downloads.wifiOnly.collectAsStateWithLifecycle()
    return pin?.let { Downloads.progressOf(it, stored, activity, net, wifiOnly) }
}

/**
 * The album and playlist pages' Download button: an outline until pinned, then a ring filling as
 * the songs arrive (dimmed while waiting for Wi-Fi or a connection), then a tick. Tapping a pinned
 * one asks before removing the download.
 */
@Composable
fun DownloadButton(item: Item, kind: PinKind, id: String, name: String) {
    val palette = Palette.current
    val pins by Downloads.pins.collectAsStateWithLifecycle()
    val pin = pins[Pin.keyOf(kind, id)]
    val progress = pinProgress(pin)
    // Not the screen's scope: leaving the page must not stop a pin, a removal or a heart half way.
    val scope = AppScope
    var confirm by remember { mutableStateOf(false) }
    val description = when {
        pin == null -> "Download"
        progress == null -> "Downloaded"
        progress.complete -> "Downloaded. Tap to remove the download"
        progress.state == PinProgress.State.Downloading -> "Downloading, ${(progress.fraction * 100).toInt()}%. Tap to remove the download"
        else -> "${progress.label}. Tap to remove the download"
    }
    IconButton(
        onClick = { if (pin == null) scope.launch { Downloads.pin(item) } else confirm = true },
        modifier = Modifier.size(48.dp).semantics { contentDescription = description },
    ) {
        when {
            pin == null -> Icon(Icons.Rounded.DownloadForOffline, contentDescription = null, tint = palette.textDim)
            progress?.complete == true -> Icon(Icons.Rounded.DownloadDone, contentDescription = null, tint = palette.accent)
            else -> Box(contentAlignment = Alignment.Center) {
                val waiting = progress?.state == PinProgress.State.WaitingForWifi || progress?.state == PinProgress.State.WaitingForNetwork
                val tint = if (waiting) palette.textFaint else palette.accent
                CircularProgressIndicator(
                    progress = { progress?.fraction ?: 0f },
                    modifier = Modifier.size(30.dp),
                    color = tint,
                    trackColor = palette.surfaceHi,
                    strokeWidth = 2.5.dp,
                )
                val icon = when (progress?.state) {
                    PinProgress.State.WaitingForWifi -> Icons.Rounded.Wifi
                    PinProgress.State.WaitingForNetwork -> Icons.Rounded.CloudOff
                    else -> Icons.Rounded.ArrowDownward
                }
                Icon(icon, contentDescription = null, tint = tint, modifier = Modifier.size(16.dp))
            }
        }
    }
    if (confirm && pin != null) {
        AlertDialog(
            onDismissRequest = { confirm = false },
            title = { Text("Remove the download?") },
            text = { Text("$name will need the network again. Songs other downloads share stay.") },
            confirmButton = {
                TextButton(onClick = { confirm = false; scope.launch { Downloads.unpin(pin.key) } }) { Text("Remove") }
            },
            dismissButton = { TextButton(onClick = { confirm = false }) { Text("Keep") } },
            containerColor = palette.surface,
        )
    }
}
