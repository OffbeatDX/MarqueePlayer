package com.offbeat.marquee.player.ui.components

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.rounded.ArrowBack
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.compositionLocalOf
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import com.offbeat.marquee.core.art.CoverSize
import com.offbeat.marquee.core.subsonic.Album
import com.offbeat.marquee.player.art.CoverImage
import com.offbeat.marquee.player.ui.theme.Palette
import com.offbeat.marquee.player.ui.theme.Tokens
import java.util.Locale

/**
 * Space a screen leaves at the top for the status bar and the cutout: screens scroll their content
 * under it (edge to edge) and pad their first item by it. The scaffold handles the sides and bottom.
 */
val LocalScreenTop = compositionLocalOf { 0.dp }

/** Whether the window is wide (rail layout): screens put headers beside their lists. */
val LocalWide = compositionLocalOf { false }

/** A shelf's or section's header: its title (opening the full list when [onOpen] is set), extras, and "See all". */
@Composable
fun SectionHeader(
    title: String,
    modifier: Modifier = Modifier,
    onOpen: (() -> Unit)? = null,
    extra: @Composable () -> Unit = {},
) {
    Row(modifier.fillMaxWidth().heightIn(min = 48.dp), verticalAlignment = Alignment.CenterVertically) {
        Row(Modifier.weight(1f), verticalAlignment = Alignment.CenterVertically) {
            Text(
                title,
                style = MaterialTheme.typography.titleLarge,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier
                    .weight(1f, fill = false)
                    .semantics { heading() }
                    .then(if (onOpen != null) Modifier.clickable(onClickLabel = "See all of $title", onClick = onOpen) else Modifier),
            )
            extra()
        }
        if (onOpen != null) SeeAll(title, onOpen)
    }
}

/** "See all": a small outlined pill, as on the desktop. */
@Composable
fun SeeAll(title: String, onClick: () -> Unit) {
    OutlinedButton(
        onClick = onClick,
        border = BorderStroke(1.dp, Palette.current.border),
        contentPadding = PaddingValues(horizontal = 14.dp),
        modifier = Modifier.heightIn(min = 48.dp).padding(vertical = 8.dp).semantics { },
        colors = ButtonDefaults.outlinedButtonColors(contentColor = Palette.current.textDim),
    ) {
        Text("See all", style = MaterialTheme.typography.labelMedium)
    }
}

/** The filled accent button (Play). */
@Composable
fun AccentButton(text: String, icon: ImageVector, onClick: () -> Unit, modifier: Modifier = Modifier, enabled: Boolean = true) {
    Button(
        onClick = onClick,
        enabled = enabled,
        modifier = modifier.heightIn(min = 48.dp),
        contentPadding = PaddingValues(start = 16.dp, end = 20.dp),
        shape = RoundedCornerShape(24.dp),
    ) {
        Icon(icon, contentDescription = null, modifier = Modifier.size(20.dp))
        Spacer(Modifier.width(8.dp))
        Text(text, style = MaterialTheme.typography.labelLarge)
    }
}

/** The quiet outlined button (Shuffle). */
@Composable
fun QuietButton(text: String, icon: ImageVector, onClick: () -> Unit, modifier: Modifier = Modifier, enabled: Boolean = true) {
    OutlinedButton(
        onClick = onClick,
        enabled = enabled,
        modifier = modifier.heightIn(min = 48.dp),
        contentPadding = PaddingValues(start = 16.dp, end = 20.dp),
        shape = RoundedCornerShape(24.dp),
        border = BorderStroke(1.dp, Palette.current.textFaint.copy(alpha = 0.5f)),
        colors = ButtonDefaults.outlinedButtonColors(contentColor = Palette.current.text),
    ) {
        Icon(icon, contentDescription = null, modifier = Modifier.size(20.dp))
        Spacer(Modifier.width(8.dp))
        Text(text, style = MaterialTheme.typography.labelLarge)
    }
}

/** A back arrow, 48 dp, for pages pushed over a tab. */
@Composable
fun BackButton(onBack: () -> Unit, modifier: Modifier = Modifier) {
    IconButton(
        onClick = onBack,
        modifier = modifier.size(48.dp).background(Palette.current.bg.copy(alpha = 0.35f), RoundedCornerShape(24.dp)),
    ) {
        Icon(Icons.AutoMirrored.Rounded.ArrowBack, contentDescription = "Back")
    }
}

/** The badge debug builds show: they never write to the server. */
@Composable
fun ReadOnlyBadge(testServer: Boolean, modifier: Modifier = Modifier) {
    Text(
        if (testServer) "Read-only · test server" else "Read-only",
        style = MaterialTheme.typography.labelMedium,
        color = Palette.current.accent,
        maxLines = 1,
        overflow = TextOverflow.Ellipsis,
        modifier = modifier
            .background(Palette.current.surfaceHi, RoundedCornerShape(999.dp))
            .padding(horizontal = 10.dp, vertical = 4.dp),
    )
}

/** A centred spinner, or an error with a retry. */
@Composable
fun LoadingOrError(error: String?, onRetry: () -> Unit, modifier: Modifier = Modifier) {
    Box(modifier.fillMaxSize().padding(24.dp), contentAlignment = Alignment.Center) {
        if (error == null) {
            CircularProgressIndicator()
        } else {
            Column(horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(error, color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodyMedium)
                TextButton(onClick = onRetry) { Text("Try again") }
            }
        }
    }
}

/** "4:05" from milliseconds; "1:02:03" past an hour. */
fun formatTime(ms: Long): String {
    val total = (ms / 1000).coerceAtLeast(0)
    val h = total / 3600
    val m = (total % 3600) / 60
    val s = total % 60
    return if (h > 0) "%d:%02d:%02d".format(h, m, s) else "%d:%02d".format(m, s)
}

/** "1.2 GB", "340 MB", "12 KB" or "512 B". */
fun formatBytes(bytes: Long): String = when {
    bytes >= 1L shl 30 -> String.format(Locale.getDefault(), "%.1f GB", bytes / (1L shl 30).toDouble())
    bytes >= 1L shl 20 -> "${bytes / (1L shl 20)} MB"
    bytes >= 1L shl 10 -> "${bytes / (1L shl 10)} KB"
    else -> "$bytes B"
}

/** "58 min" or "1 h 12 min" from seconds. */
fun formatLength(seconds: Int): String {
    val minutes = (seconds + 30) / 60
    return if (minutes >= 60) "${minutes / 60} h ${minutes % 60} min" else "$minutes min"
}

/** A fixed-width space for album tiles in a row. */
val RowTileWidth: Dp = 140.dp
