package com.offbeat.marquee.player.ui.components

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.max
import androidx.compose.ui.unit.min
import com.offbeat.marquee.player.art.Backdrop
import com.offbeat.marquee.player.art.CoverImage
import com.offbeat.marquee.player.ui.theme.Palette
import com.offbeat.marquee.player.ui.theme.Tokens

/**
 * A page of songs under a cover: albums, playlists, favourite songs. Portrait: the cover, the
 * [info] block and [buttons] over the blurred art, then the [rows]. Wide: the header in a pane on
 * the left that always fits its height (on a short window the cover sits beside the text, and the
 * pane scrolls if it still can't fit), the buttons heading the rows on the right, where they stay
 * in view.
 */
@Composable
fun CollectionPage(
    coverArt: String?,
    coverDescription: String,
    onBack: () -> Unit,
    info: @Composable (wide: Boolean) -> Unit,
    buttons: @Composable (Modifier) -> Unit,
    rows: LazyListScope.() -> Unit,
    coverModifier: Modifier = Modifier,
) {
    val top = LocalScreenTop.current
    val palette = Palette.current
    // A list state per layout, both kept across rotations, so turning the phone and back returns to
    // the same place. When the place is in the rows (index 1 on in both: the first item is the header
    // in portrait, the buttons in landscape), the other layout opens at the same row.
    val compactList = rememberKeptListState()
    val wideList = rememberKeptListState()
    val wide = LocalWide.current
    LaunchedEffect(wide) {
        val (from, to) = if (wide) compactList to wideList else wideList to compactList
        if (from.firstVisibleItemIndex >= 1) to.scrollToItem(from.firstVisibleItemIndex, from.firstVisibleItemScrollOffset)
    }
    Box(Modifier.fillMaxSize()) {
        if (wide) {
            Row(Modifier.fillMaxSize()) {
                BoxWithConstraints(Modifier.weight(0.42f).fillMaxHeight()) {
                    Backdrop(coverArt, palette.backdropTint, Modifier.matchParentSize())
                    // Below the back button; what is left decides the layout.
                    val headerTop = top + 60.dp
                    val paneWidth = maxWidth
                    val room = maxHeight - headerTop - 16.dp
                    val short = room < 420.dp
                    Column(
                        Modifier.fillMaxSize().verticalScroll(rememberScrollState())
                            .padding(start = Tokens.PageMargin, end = Tokens.PageMargin, top = headerTop, bottom = 16.dp),
                    ) {
                        if (short) {
                            val size = clampDp(min(room, (paneWidth - Tokens.PageMargin * 2) * 0.42f), 72.dp, 180.dp)
                            Row(horizontalArrangement = Arrangement.spacedBy(14.dp)) {
                                Cover(coverArt, coverDescription, size, coverModifier)
                                Column(Modifier.weight(1f)) { info(true) }
                            }
                        } else {
                            val size = clampDp(min(paneWidth - Tokens.PageMargin * 2, room - 200.dp), 96.dp, 300.dp)
                            Cover(coverArt, coverDescription, size, coverModifier)
                            Column(Modifier.padding(top = 14.dp)) { info(true) }
                        }
                    }
                }
                LazyColumn(
                    state = wideList,
                    modifier = Modifier.weight(0.58f).fillMaxHeight(),
                    contentPadding = PaddingValues(top = top + 8.dp, bottom = 24.dp),
                ) {
                    item(key = "buttons", contentType = "buttons") {
                        buttons(Modifier.padding(horizontal = Tokens.PageMargin, vertical = 4.dp))
                    }
                    rows()
                }
            }
        } else BoxWithConstraints(Modifier.fillMaxSize()) {
            // The cover also shrinks with the height left, so the buttons stay in the first screen:
            // on a 360 x 640 dp phone (bar, mini player, three-button navigation) the cover alone
            // filled it. About 250 dp is kept for the title lines and the buttons.
            val coverCap = max(120.dp, maxHeight - top - 56.dp - 250.dp)
            LazyColumn(Modifier.fillMaxSize(), state = compactList, contentPadding = PaddingValues(bottom = 24.dp)) {
                item(key = "header", contentType = "header") {
                    Box {
                        Backdrop(coverArt, palette.backdropTint, Modifier.matchParentSize())
                        Box(Modifier.matchParentSize().background(Brush.verticalGradient(0.6f to Color.Transparent, 1f to palette.bg)))
                        BoxWithConstraints(Modifier.padding(top = top + 56.dp).padding(horizontal = Tokens.PageMargin)) {
                            val width = maxWidth
                            Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                                Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.Center) {
                                    Cover(coverArt, coverDescription, min(min(width * 0.72f, 300.dp), coverCap), coverModifier)
                                }
                                Column(Modifier.padding(top = 14.dp)) { info(false) }
                                buttons(Modifier.padding(top = 10.dp, bottom = 8.dp))
                            }
                        }
                    }
                }
                rows()
            }
        }
        BackButton(onBack, Modifier.padding(top = top + 4.dp, start = 8.dp))
    }
}

private fun clampDp(v: Dp, lo: Dp, hi: Dp): Dp = max(lo, min(v, hi))

@Composable
private fun Cover(coverArt: String?, description: String, size: Dp, modifier: Modifier) {
    CoverImage(
        coverArt,
        modifier.size(size),
        shape = RoundedCornerShape(Tokens.RadiusCard),
        contentDescription = description,
    )
}
