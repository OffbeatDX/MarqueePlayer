package com.offbeat.marquee.player.ui.settings

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.produceState
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.offbeat.marquee.player.R
import com.offbeat.marquee.player.ui.components.BackButton
import com.offbeat.marquee.player.ui.components.LocalScreenTop
import com.offbeat.marquee.player.ui.theme.Palette
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

/** A library the app ships, with its licence. */
private data class Library(val name: String, val by: String, val licence: String)

private val LIBRARIES = listOf(
    Library("Jetpack Compose (UI, Foundation, Material 3, Material icons)", "Google / The Android Open Source Project", "Apache License 2.0"),
    Library("AndroidX Activity, Core, Lifecycle, Navigation", "The Android Open Source Project", "Apache License 2.0"),
    Library("AndroidX Media3 (ExoPlayer, Session, OkHttp data source)", "The Android Open Source Project", "Apache License 2.0"),
    Library("AndroidX WorkManager, Glance", "The Android Open Source Project", "Apache License 2.0"),
    Library("Kotlin standard library, kotlinx.coroutines, kotlinx.serialization", "JetBrains", "Apache License 2.0"),
    Library("OkHttp, Okio", "Square", "Apache License 2.0"),
    Library("Coil", "Coil contributors", "Apache License 2.0"),
)

/** A licence whose full text the screen can show, from a raw resource. */
private enum class LicenceText(val title: String, val res: Int) {
    Gpl("GNU General Public License, version 3", R.raw.gpl_3_0),
    Apache("Apache License 2.0", R.raw.apache_license_2_0),
}

/**
 * Settings > About > Open-source licences: the app's own licence (GPL-3.0-or-later), the libraries
 * in the app and their licences, and the GPL and the Apache License 2.0 (which the libraries use)
 * in full.
 */
@Composable
fun LicencesScreen(onBack: () -> Unit) {
    val palette = Palette.current
    val context = LocalContext.current
    var shown by rememberSaveable { mutableStateOf<LicenceText?>(null) }
    val texts by produceState<Map<LicenceText, String>>(emptyMap()) {
        value = withContext(Dispatchers.IO) {
            LicenceText.entries.associateWith { context.resources.openRawResource(it.res).bufferedReader().use { r -> r.readText() } }
        }
    }
    Box(Modifier.fillMaxSize(), contentAlignment = Alignment.TopCenter) {
        LazyColumn(
            Modifier.widthIn(max = SettingsMaxWidth).fillMaxSize(),
            contentPadding = PaddingValues(top = LocalScreenTop.current + 4.dp, bottom = 32.dp),
        ) {
            item {
                Row(Modifier.padding(start = 8.dp, bottom = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                    BackButton(onBack)
                    Text(
                        "Open-source licences",
                        style = MaterialTheme.typography.headlineSmall,
                        modifier = Modifier.padding(start = 12.dp).semantics { heading() },
                    )
                }
            }
            item {
                Column(Modifier.padding(horizontal = 16.dp)) {
                    SettingCard("This app") {
                        SettingRow(
                            name = "Marquee Player",
                            hint = "Offbeat · GNU General Public License, version 3 or later (GPL-3.0-or-later)",
                        )
                    }
                }
            }
            item {
                Column(Modifier.padding(start = 16.dp, end = 16.dp, top = SectionGap)) {
                    SettingCard("Libraries") {
                        LIBRARIES.forEachIndexed { i, lib ->
                            if (i > 0) RowDivider()
                            SettingRow(name = lib.name, hint = "${lib.by} · ${lib.licence}")
                        }
                    }
                }
            }
            item {
                Column(Modifier.padding(start = 16.dp, end = 16.dp, top = SectionGap)) {
                    SettingCard("Licence texts") {
                        LicenceText.entries.forEachIndexed { i, licence ->
                            if (i > 0) RowDivider()
                            NavSettingRow(
                                name = licence.title,
                                hint = if (shown == licence) "Tap to hide the full text" else "Tap for the full text",
                                onClick = { shown = if (shown == licence) null else licence },
                            )
                        }
                    }
                }
            }
            shown?.let { which ->
                val paragraphs = texts[which]?.split(Regex("\\n\\s*\\n"))?.map { it.trim() }?.filter { it.isNotEmpty() }.orEmpty()
                items(paragraphs) { p ->
                    Text(
                        p.lines().joinToString(" ") { it.trim() },
                        style = MaterialTheme.typography.bodySmall.copy(fontFamily = FontFamily.Default, lineHeight = 18.sp),
                        color = palette.textDim,
                        modifier = Modifier.fillMaxWidth().padding(horizontal = 20.dp, vertical = 6.dp),
                    )
                }
            }
        }
    }
}
