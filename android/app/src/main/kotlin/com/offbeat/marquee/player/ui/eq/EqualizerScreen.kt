package com.offbeat.marquee.player.ui.eq

import android.content.Context
import android.net.Uri
import android.provider.OpenableColumns
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.Close
import androidx.compose.material3.FilterChip
import androidx.compose.material3.FilterChipDefaults
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.min
import androidx.lifecycle.ViewModel
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import kotlinx.coroutines.flow.MutableStateFlow
import com.offbeat.marquee.core.eq.Apo
import com.offbeat.marquee.core.eq.EqMode
import com.offbeat.marquee.core.eq.EqProfile
import com.offbeat.marquee.core.eq.EqSetting
import com.offbeat.marquee.core.eq.Graphic
import com.offbeat.marquee.core.eq.maxBoostDb
import com.offbeat.marquee.player.eq.EqStore
import com.offbeat.marquee.player.ui.components.BackButton
import com.offbeat.marquee.player.ui.components.LocalScreenTop
import com.offbeat.marquee.player.ui.theme.Palette
import com.offbeat.marquee.player.ui.theme.Tokens
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/**
 * Settings > Equalizer (also from Now Playing's ⋯). Off by default. The on/off switch with its battery
 * note, the curve, then either the 10-band graphic EQ (presets, sliders) or the one imported AutoEq /
 * Equalizer APO profile. Changes apply live while a slider moves; the setting is saved when it stops.
 *
 * Portrait: the curve, then the sliders. Landscape (or any window wider than tall): the curve on the
 * left, the presets and sliders on the right, so the sliders always fit without scrolling.
 */
@Composable
fun EqualizerScreen(onBack: () -> Unit) {
    val s by EqStore.setting.collectAsStateWithLifecycle()
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val clipboard = LocalClipboardManager.current
    // In a ViewModel (this screen's back stack entry): it survives rotation, which recreates the activity.
    val vm: EqScreenModel = viewModel()
    val error by vm.error.collectAsStateWithLifecycle()

    fun importText(text: String?, name: String) {
        if (text.isNullOrBlank()) { vm.error.value = "Nothing to import: the ${if (name == PASTED) "clipboard" else "file"} is empty."; return }
        when (val r = Apo.read(text)) {
            is Apo.Result.Ok -> {
                vm.error.value = null
                val p = r.profile
                EqStore.update { it.copy(mode = EqMode.Profile, profile = EqProfile(name, p.bands, p.preampDb, p.notes)) }
            }
            is Apo.Result.Error -> vm.error.value = r.message
        }
    }
    val picker = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
        if (uri != null) scope.launch {
            val read = withContext(Dispatchers.IO) { readDocument(context, uri) }
            if (read == null) vm.error.value = "Couldn't read that file." else importText(read.second, Apo.nameFromFile(read.first))
        }
    }
    val actions = ImportActions(
        pick = { picker.launch(arrayOf("text/plain")) },
        // The clipboard is read only here, when Paste is tapped.
        paste = { importText(clipboard.getText()?.text, PASTED) },
    )

    BoxWithConstraints(Modifier.fillMaxSize().padding(top = LocalScreenTop.current)) {
        val landscape = maxWidth > maxHeight && maxWidth >= 560.dp
        val short = maxHeight < 640.dp
        val boxHeight = maxHeight
        // The least height that leaves the sliders about 150 dp of travel: what is drawn at a fixed
        // size (the curve, the slider track) plus the text, chips and buttons, which grow with the
        // font size. Shorter windows scroll; taller ones are unchanged.
        val fontScale = LocalDensity.current.fontScale.coerceAtLeast(1f)
        val minPortrait = 300.dp + 340.dp * fontScale
        val minLandscape = 160.dp + 180.dp * fontScale
        if (landscape) {
            Row(Modifier.fillMaxSize().padding(end = Tokens.PageMargin, bottom = 8.dp), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                Column(Modifier.weight(0.85f).fillMaxHeight()) {
                    Header(onBack)
                    OnOffRow(s, compact = true)
                    // No taller than its width allows (a tablet in landscape drew it 200 x 500 dp).
                    BoxWithConstraints(Modifier.weight(1f).fillMaxWidth().padding(start = Tokens.PageMargin, top = 4.dp)) {
                        EqCurve(s.bands, s.enabled, Modifier.fillMaxWidth().height(min(maxHeight, maxWidth / 1.3f)))
                    }
                    PreampLine(s, Modifier.padding(start = Tokens.PageMargin, top = 2.dp))
                }
                // Scrolls when shorter than minLandscape: a small phone in landscape (360 dp tall, less
                // the status bar and mini player) left the sliders about 25 dp of travel.
                Column(
                    Modifier.weight(1.5f).verticalScroll(rememberScrollState())
                        .height(maxOf(boxHeight - 8.dp, minLandscape)).padding(top = 4.dp),
                ) {
                    error?.let { ErrorCard(it, { vm.error.value = null }, Modifier.padding(bottom = 4.dp)) }
                    if (s.playsProfile) ProfilePanel(s, actions, Modifier.weight(1f))
                    else GraphicPanel(s, actions, sliderPad = 0.dp, compact = true)
                }
            }
        } else {
            // Scrolls when shorter than minPortrait: the sliders take what is left under the curve,
            // presets and import row, which on a small phone (360 x 760 dp, with the bar and mini
            // player), or at font size 1.3, was 30 to 35 dp of travel.
            Column(
                Modifier.fillMaxWidth().verticalScroll(rememberScrollState())
                    .height(maxOf(boxHeight, minPortrait)).padding(bottom = 8.dp),
            ) {
                Header(onBack)
                OnOffRow(s, compact = false)
                EqCurve(
                    s.bands, s.enabled,
                    Modifier.fillMaxWidth().height(if (short) 110.dp else 150.dp).padding(horizontal = Tokens.PageMargin),
                )
                PreampLine(s, Modifier.padding(horizontal = Tokens.PageMargin, vertical = 2.dp))
                error?.let { ErrorCard(it, { vm.error.value = null }, Modifier.padding(horizontal = Tokens.PageMargin, vertical = 4.dp)) }
                if (s.playsProfile) ProfilePanel(s, actions, Modifier.weight(1f).padding(horizontal = Tokens.PageMargin))
                else GraphicPanel(s, actions, sliderPad = 8.dp, compact = false)
            }
        }
    }
}

private const val PASTED = "Pasted profile"

/** What the screen keeps across a rotation: the last import's error. */
class EqScreenModel : ViewModel() {
    val error = MutableStateFlow<String?>(null)
}

private class ImportActions(val pick: () -> Unit, val paste: () -> Unit)

@Composable
private fun Header(onBack: () -> Unit) {
    Row(Modifier.padding(start = 8.dp, top = 4.dp), verticalAlignment = Alignment.CenterVertically) {
        BackButton(onBack)
        Text(
            "Equalizer",
            style = MaterialTheme.typography.headlineSmall,
            modifier = Modifier.padding(start = 12.dp).semantics { heading() },
        )
    }
}

/** The switch, with what it costs, and that a system equalizer app works too. */
@Composable
private fun OnOffRow(s: EqSetting, compact: Boolean) {
    val palette = Palette.current
    Row(
        Modifier
            .fillMaxWidth()
            .heightIn(min = 64.dp)
            .clickable(role = Role.Switch, onClickLabel = if (s.enabled) "Turn off" else "Turn on") { EqStore.update { it.copy(enabled = !it.enabled) } }
            .padding(horizontal = Tokens.PageMargin, vertical = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Column(Modifier.weight(1f).padding(end = 12.dp)) {
            Text(if (s.enabled) "On · ${s.label}" else "Off", style = MaterialTheme.typography.bodyLarge, color = if (s.enabled) palette.accent else palette.text)
            Text("Uses more battery: audio offload is off while the equalizer is on.", style = MaterialTheme.typography.bodySmall, color = palette.textDim)
            if (!compact) {
                Text(
                    "Off by default. A system equalizer app (such as Wavelet) also works with Marquee.",
                    style = MaterialTheme.typography.bodySmall,
                    color = palette.textFaint,
                )
            }
        }
        Switch(checked = s.enabled, onCheckedChange = null)
    }
}

@Composable
private fun PreampLine(s: EqSetting, modifier: Modifier) {
    val db = if (s.playsProfile) s.profile?.preampDb ?: -maxBoostDb(s.bands, 48_000) else -maxBoostDb(s.bands, 48_000)
    val how = if (s.playsProfile && s.profile?.preampDb != null) "from the profile" else "automatic headroom"
    Text(
        "Preamp ${dbLabel(db)} dB · $how",
        style = MaterialTheme.typography.labelSmall,
        color = Palette.current.textFaint,
        modifier = modifier,
    )
}

@Composable
private fun ColumnScope.GraphicPanel(s: EqSetting, actions: ImportActions, sliderPad: Dp, compact: Boolean) {
    val palette = Palette.current
    val preset = Graphic.presetOf(s.gains)
    Row(Modifier.padding(horizontal = Tokens.PageMargin).padding(top = 4.dp), verticalAlignment = Alignment.CenterVertically) {
        Text("Preset", style = MaterialTheme.typography.labelMedium, color = palette.textDim)
        Text(
            preset?.name ?: "Custom",
            style = MaterialTheme.typography.labelLarge,
            color = palette.text,
            modifier = Modifier.padding(start = 8.dp).semantics { liveRegion = LiveRegionMode.Polite },
        )
    }
    LazyRow(
        contentPadding = PaddingValues(horizontal = Tokens.PageMargin),
        horizontalArrangement = Arrangement.spacedBy(8.dp),
        modifier = Modifier.fillMaxWidth(),
    ) {
        items(Graphic.PRESETS, key = { it.name }) { p ->
            FilterChip(
                selected = p === preset,
                onClick = { EqStore.update { it.copy(gains = p.gains) } },
                label = { Text(p.name) },
                modifier = Modifier.heightIn(min = 48.dp),
                colors = FilterChipDefaults.filterChipColors(
                    selectedContainerColor = palette.accent.copy(alpha = 0.18f),
                    selectedLabelColor = palette.accent,
                ),
            )
        }
    }
    Row(Modifier.weight(1f).fillMaxWidth().padding(horizontal = sliderPad).padding(top = 4.dp)) {
        for (i in 0 until Graphic.COUNT) {
            BandSlider(
                label = Graphic.LABELS[i],
                spoken = Graphic.SPOKEN[i],
                value = s.gains[i],
                onChange = { v -> EqStore.update(persist = false) { it.copy(gains = it.gains.toMutableList().also { g -> g[i] = v }) } },
                onDone = { EqStore.save() },
                on = s.enabled,
                modifier = Modifier.weight(1f).fillMaxHeight(),
            )
        }
    }
    ImportRow(s, actions, compact, Modifier.padding(horizontal = Tokens.PageMargin))
}

@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun ImportRow(s: EqSetting, actions: ImportActions, compact: Boolean, modifier: Modifier) {
    FlowRow(modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.Center) {
        OutlinedButton(onClick = actions.pick, modifier = Modifier.heightIn(min = 48.dp)) {
            Text(if (compact) "Import AutoEq / APO profile" else "Import AutoEq / Equalizer APO profile", maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
        TextButton(onClick = actions.paste, modifier = Modifier.heightIn(min = 48.dp)) { Text("Paste") }
        s.profile?.let { p ->
            TextButton(onClick = { EqStore.update { it.copy(mode = EqMode.Profile) } }, modifier = Modifier.heightIn(min = 48.dp)) {
                Text("Use “${p.name}”", maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
        }
    }
}

/** The imported profile: its name (editable), what it holds, what was left out, and the way back. */
@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun ProfilePanel(s: EqSetting, actions: ImportActions, modifier: Modifier) {
    val palette = Palette.current
    val p = s.profile ?: return
    Column(modifier.fillMaxWidth().verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Text("AUTOEQ / EQUALIZER APO PROFILE", style = MaterialTheme.typography.labelSmall, color = palette.textDim, modifier = Modifier.padding(top = 8.dp))
        OutlinedTextField(
            value = p.name,
            onValueChange = { name -> EqStore.update { it.copy(profile = it.profile?.copy(name = name.take(80))) } },
            label = { Text("Name") },
            singleLine = true,
            modifier = Modifier.fillMaxWidth(),
        )
        val pre = p.preampDb?.let { "preamp ${dbLabel(it)} dB" } ?: "automatic preamp"
        Text("${p.bands.size} ${if (p.bands.size == 1) "filter" else "filters"} · $pre", style = MaterialTheme.typography.bodyMedium)
        if (p.notes.isNotEmpty()) {
            Text("Read in a particular way or left out:", style = MaterialTheme.typography.labelMedium, color = palette.textDim)
            for (n in p.notes.take(12)) Text(n, style = MaterialTheme.typography.bodySmall, color = palette.textDim)
            if (p.notes.size > 12) Text("… and ${p.notes.size - 12} more", style = MaterialTheme.typography.bodySmall, color = palette.textFaint)
        }
        FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            OutlinedButton(onClick = { EqStore.update { it.copy(mode = EqMode.Graphic) } }, modifier = Modifier.heightIn(min = 48.dp)) { Text("Back to graphic EQ") }
            TextButton(onClick = actions.pick, modifier = Modifier.heightIn(min = 48.dp)) { Text("Import another") }
            TextButton(onClick = actions.paste, modifier = Modifier.heightIn(min = 48.dp)) { Text("Paste") }
        }
        Text("One profile is kept: importing another replaces it.", style = MaterialTheme.typography.bodySmall, color = palette.textFaint)
        Spacer(Modifier.height(8.dp))
    }
}

@Composable
private fun ErrorCard(message: String, onDismiss: () -> Unit, modifier: Modifier) {
    Row(
        modifier
            .fillMaxWidth()
            .background(Tokens.Error.copy(alpha = 0.12f), RoundedCornerShape(Tokens.RadiusCard))
            .padding(start = 12.dp)
            .semantics { liveRegion = LiveRegionMode.Assertive },
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Column(Modifier.weight(1f).padding(vertical = 8.dp)) {
            Text("Couldn't import this profile", style = MaterialTheme.typography.labelLarge, color = Tokens.Error)
            Text(message, style = MaterialTheme.typography.bodySmall, color = Palette.current.text)
        }
        IconButton(onClick = onDismiss, modifier = Modifier.width(48.dp)) { Icon(Icons.Rounded.Close, contentDescription = "Dismiss") }
    }
}

/** The document's display name and text (at most [Apo.MAX_CHARS] + 1 characters, so a huge file is refused as too long). */
private fun readDocument(context: Context, uri: Uri): Pair<String, String>? = runCatching {
    val name = context.contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { c ->
        if (c.moveToFirst()) c.getString(0) else null
    } ?: "Imported profile"
    val text = context.contentResolver.openInputStream(uri)?.use { input ->
        val reader = input.reader(Charsets.UTF_8)
        val buf = CharArray(Apo.MAX_CHARS + 1)
        var n = 0
        while (n < buf.size) { val r = reader.read(buf, n, buf.size - n); if (r < 0) break; n += r }
        String(buf, 0, n)
    } ?: return null
    name to text
}.getOrNull()
