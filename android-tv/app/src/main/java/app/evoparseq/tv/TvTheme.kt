package app.evoparseq.tv

import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.graphics.Color
import androidx.tv.material3.LocalContentColor
import androidx.tv.material3.MaterialTheme
import androidx.tv.material3.darkColorScheme

/** One Android presentation palette; no account/session policy belongs here. */
@Composable
internal fun TvTheme(content: @Composable () -> Unit) {
    MaterialTheme(colorScheme = darkColorScheme(
        primary = Color(0xFF71BCFF), onPrimary = Color(0xFF08203A),
        background = Color(0xFF10141C), onBackground = Color(0xFFEAF1FA),
        surface = Color(0xFF202936), onSurface = Color(0xFFEAF1FA),
        surfaceVariant = Color(0xFF293448), onSurfaceVariant = Color(0xFFD2DFEF),
        inverseSurface = Color(0xFFEAF1FA), inverseOnSurface = Color(0xFF10141C),
    )) {
        // MaterialTheme supplies a palette, not a page's content color. Without
        // a Surface/provider, TV Text defaults to black even in a dark theme.
        // Buttons/cards retain their nested state-specific providers, including
        // dark text on the bright focused surface: do not force every Text white.
        CompositionLocalProvider(LocalContentColor provides MaterialTheme.colorScheme.onBackground) {
            content()
        }
    }
}
