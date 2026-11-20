package com.offbeat.marquee.player.ui.theme

import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Shapes
import androidx.compose.material3.Typography
import androidx.compose.material3.darkColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.ReadOnlyComposable
import androidx.compose.runtime.remember
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.offbeat.marquee.core.art.Accent

/**
 * The desktop's tokens (crates/app/ui/theme.slint), with its type scale brought down to phone
 * sizes. The accent follows the playing cover; pure black turns the background off for OLED, with
 * surfaces lifted a little so cards still read.
 */
object Tokens {
    val Bg = Color(0xFF0C0B10)
    val Surface = Color(0xFF17151E)
    val SurfaceHi = Color(0xFF221F2B)
    val Border = Color(0x14FFFFFF)
    val Text = Color(0xFFF3F0F7)
    val TextDim = Color(0xFFA9A3B6)
    val TextFaint = Color(0xFF928DA0)
    val Hover = Color(0x10FFFFFF)
    val Press = Color(0x1CFFFFFF)
    val Error = Color(0xFFF2A0A8)

    // Pure black: the background off, surfaces a step above it.
    val BlackBg = Color(0xFF000000)
    val BlackSurface = Color(0xFF111015)
    val BlackSurfaceHi = Color(0xFF1C1A23)

    val RadiusCover = 8.dp
    val RadiusCard = 12.dp
    val RadiusControl = 8.dp

    /** Room around page content (the desktop's 40 px page margin, phone-sized). */
    val PageMargin = 16.dp

    /** The tint over blurred art behind pages, mixed toward the background. */
    val BackdropTint = Color(0xB00C0B10)
}

/** The colours the app's own components read (Material's scheme is derived from it). */
@Immutable
data class Palette(
    val bg: Color,
    val surface: Color,
    val surfaceHi: Color,
    val border: Color,
    val text: Color,
    val textDim: Color,
    val textFaint: Color,
    val accent: Color,
    val onAccent: Color,
    val pureBlack: Boolean,
) {
    /** The tint drawn over a blurred backdrop. */
    val backdropTint: Color get() = if (pureBlack) Color(0xC8000000) else Tokens.BackdropTint

    companion object {
        val current: Palette
            @Composable @ReadOnlyComposable get() = LocalPalette.current

        fun of(accentRgb: Int, pureBlack: Boolean) = Palette(
            bg = if (pureBlack) Tokens.BlackBg else Tokens.Bg,
            surface = if (pureBlack) Tokens.BlackSurface else Tokens.Surface,
            surfaceHi = if (pureBlack) Tokens.BlackSurfaceHi else Tokens.SurfaceHi,
            border = Tokens.Border,
            text = Tokens.Text,
            textDim = Tokens.TextDim,
            textFaint = Tokens.TextFaint,
            accent = Color(0xFF000000 or accentRgb.toLong()),
            onAccent = Color(0xFF000000 or Accent.onAccent(accentRgb).toLong()),
            pureBlack = pureBlack,
        )
    }
}

val LocalPalette = staticCompositionLocalOf { Palette.of(Accent.FALLBACK, false) }

/** The desktop's type scale (13 / 14 / 16 / 22 / 40 px) brought to phone sizes. */
private val typography = Typography().run {
    copy(
        displaySmall = TextStyle(fontSize = 28.sp, lineHeight = 34.sp, fontWeight = FontWeight.ExtraBold, letterSpacing = (-0.2).sp),
        headlineMedium = TextStyle(fontSize = 26.sp, lineHeight = 32.sp, fontWeight = FontWeight.ExtraBold),
        headlineSmall = TextStyle(fontSize = 22.sp, lineHeight = 28.sp, fontWeight = FontWeight.Bold),
        titleLarge = TextStyle(fontSize = 19.sp, lineHeight = 24.sp, fontWeight = FontWeight.Bold),
        titleMedium = TextStyle(fontSize = 16.sp, lineHeight = 22.sp, fontWeight = FontWeight.SemiBold),
        titleSmall = TextStyle(fontSize = 14.sp, lineHeight = 20.sp, fontWeight = FontWeight.SemiBold),
        bodyLarge = TextStyle(fontSize = 16.sp, lineHeight = 22.sp),
        bodyMedium = TextStyle(fontSize = 14.sp, lineHeight = 20.sp),
        bodySmall = TextStyle(fontSize = 13.sp, lineHeight = 18.sp),
        labelLarge = TextStyle(fontSize = 14.sp, lineHeight = 20.sp, fontWeight = FontWeight.SemiBold),
        labelMedium = TextStyle(fontSize = 12.sp, lineHeight = 16.sp, fontWeight = FontWeight.SemiBold),
        // The desktop's small capitals over titles ("ALBUM", the hero's shelf name).
        labelSmall = TextStyle(fontSize = 11.sp, lineHeight = 14.sp, fontWeight = FontWeight.Bold, letterSpacing = 1.5.sp),
    )
}

private val shapes = Shapes(
    extraSmall = RoundedCornerShape(Tokens.RadiusControl),
    small = RoundedCornerShape(Tokens.RadiusControl),
    medium = RoundedCornerShape(Tokens.RadiusCard),
    large = RoundedCornerShape(Tokens.RadiusCard),
    extraLarge = RoundedCornerShape(20.dp),
)

@Composable
fun MarqueeTheme(accentRgb: Int = Accent.FALLBACK, pureBlack: Boolean = false, content: @Composable () -> Unit) {
    val palette = remember(accentRgb, pureBlack) { Palette.of(accentRgb, pureBlack) }
    val colors = remember(palette) {
        darkColorScheme(
            primary = palette.accent,
            onPrimary = palette.onAccent,
            primaryContainer = palette.surfaceHi,
            onPrimaryContainer = palette.text,
            secondary = palette.accent,
            onSecondary = palette.onAccent,
            secondaryContainer = palette.surfaceHi,
            onSecondaryContainer = palette.text,
            tertiary = palette.accent,
            background = palette.bg,
            onBackground = palette.text,
            surface = palette.bg,
            onSurface = palette.text,
            surfaceVariant = palette.surface,
            onSurfaceVariant = palette.textDim,
            surfaceContainerLowest = palette.bg,
            surfaceContainerLow = palette.surface,
            surfaceContainer = palette.surface,
            surfaceContainerHigh = palette.surfaceHi,
            surfaceContainerHighest = palette.surfaceHi,
            // Snackbars (and plain tooltips) draw on the inverse colours: dark here, like the rest.
            inverseSurface = palette.surfaceHi,
            inverseOnSurface = palette.text,
            inversePrimary = palette.accent,
            outline = palette.textFaint,
            outlineVariant = palette.border,
            error = Tokens.Error,
            onError = palette.onAccent,
            scrim = Color(0xB3000000),
        )
    }
    CompositionLocalProvider(LocalPalette provides palette) {
        MaterialTheme(colorScheme = colors, shapes = shapes, typography = typography, content = content)
    }
}
