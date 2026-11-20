package com.offbeat.marquee.player.ui

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.slideOutVertically
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.consumeWindowInsets
import androidx.compose.foundation.layout.WindowInsetsSides
import androidx.compose.foundation.layout.asPaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.only
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawing
import androidx.compose.foundation.layout.systemBars
import androidx.compose.foundation.layout.displayCutout
import androidx.compose.foundation.layout.union
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.windowInsetsBottomHeight
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.CloudOff
import androidx.compose.material.icons.rounded.Home
import androidx.compose.material.icons.rounded.LibraryMusic
import androidx.compose.material.icons.rounded.Search
import androidx.compose.material3.Icon
import androidx.compose.material3.NavigationBar
import androidx.compose.material3.NavigationBarItem
import androidx.compose.material3.NavigationBarItemDefaults
import androidx.compose.material3.NavigationRail
import androidx.compose.material3.NavigationRailItem
import androidx.compose.material3.NavigationRailItemDefaults
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.layout.Layout
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.unit.Constraints
import com.offbeat.marquee.player.ui.components.ThemedSnackbarHost
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.unit.dp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.repeatOnLifecycle
import androidx.navigation.NavDestination.Companion.hasRoute
import androidx.navigation.NavGraph.Companion.findStartDestination
import androidx.navigation.NavHostController
import androidx.navigation.compose.NavHost
import androidx.navigation.compose.composable
import androidx.navigation.compose.rememberNavController
import androidx.navigation.toRoute
import com.offbeat.marquee.core.library.ShelfId
import com.offbeat.marquee.player.data.Library
import com.offbeat.marquee.player.net.NetworkMonitor
import com.offbeat.marquee.player.offline.Availability
import com.offbeat.marquee.player.offline.Downloads
import com.offbeat.marquee.player.offline.LocalOffline
import com.offbeat.marquee.player.offline.OfflineView
import com.offbeat.marquee.player.ui.downloads.DownloadsScreen
import com.offbeat.marquee.player.playback.PlayerConnection
import com.offbeat.marquee.player.session.SessionState
import com.offbeat.marquee.player.ui.album.AlbumScreen
import com.offbeat.marquee.player.ui.artist.ArtistScreen
import com.offbeat.marquee.player.ui.components.LocalScreenTop
import com.offbeat.marquee.player.ui.components.LocalWide
import com.offbeat.marquee.player.ui.actions.ActionNavigation
import com.offbeat.marquee.player.ui.actions.ActionRequest
import com.offbeat.marquee.player.ui.actions.ActionSheet
import com.offbeat.marquee.player.ui.actions.ActionRequestSaver
import com.offbeat.marquee.player.ui.actions.LocalItemActions
import com.offbeat.marquee.player.ui.library.FavouriteSongsScreen
import com.offbeat.marquee.player.ui.library.GenreScreen
import com.offbeat.marquee.player.ui.library.LibraryScreen
import com.offbeat.marquee.player.ui.library.PlaylistScreen
import com.offbeat.marquee.player.ui.search.SearchAllScreen
import com.offbeat.marquee.player.ui.search.SearchScreen
import com.offbeat.marquee.player.ui.home.HomeScreen
import com.offbeat.marquee.player.ui.home.ShelfGridScreen
import com.offbeat.marquee.player.ui.player.MiniPlayer
import com.offbeat.marquee.player.ui.player.NowPlayingScreen
import com.offbeat.marquee.player.ui.settings.LicencesScreen
import com.offbeat.marquee.player.ui.settings.SettingsScreen
import com.offbeat.marquee.player.ui.settings.ShelvesScreen
import com.offbeat.marquee.player.ui.eq.EqualizerScreen
import com.offbeat.marquee.player.ui.theme.Palette
import kotlinx.serialization.Serializable
import kotlin.reflect.KClass
import com.offbeat.marquee.player.UiRequests
import com.offbeat.marquee.player.sync.Sync
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.material3.MaterialTheme

// ---- Routes ---------------------------------------------------------------------------------

@Serializable data object HomeRoute
@Serializable data object LibraryRoute
@Serializable data object SearchRoute
@Serializable data object SettingsRoute
@Serializable data object EqualizerRoute
@Serializable data object ShelvesRoute
@Serializable data object LicencesRoute
@Serializable data class AlbumRoute(val id: String)
@Serializable data class ArtistRoute(val id: String)
@Serializable data class ShelfRoute(val shelf: String)
@Serializable data class PlaylistRoute(val id: String)
@Serializable data class GenreRoute(val name: String, val albumCount: Int, val songCount: Int)
@Serializable data object FavouriteSongsRoute
@Serializable data object DownloadsRoute
@Serializable data class SearchAllRoute(val query: String, val kind: String)

private enum class Tab(val label: String, val icon: ImageVector, val route: Any, val routeClass: KClass<*>) {
    Home("Home", Icons.Rounded.Home, HomeRoute, HomeRoute::class),
    Library("Library", Icons.Rounded.LibraryMusic, LibraryRoute, LibraryRoute::class),
    Search("Search", Icons.Rounded.Search, SearchRoute, SearchRoute::class),
}

/**
 * The signed-in app. Width decides the layout: a bottom bar under the mini player when compact, a
 * navigation rail beside the content (mini player along its bottom) from 600 dp. Now Playing is a
 * full-screen sheet over everything.
 */
@Composable
fun SignedInApp(session: SessionState.SignedIn, player: PlayerConnection, onSignOut: () -> Unit) {
    val nav = rememberNavController()
    val np by player.state.collectAsStateWithLifecycle()
    val queue by player.queue.collectAsStateWithLifecycle()
    var nowPlayingOpen by rememberSaveable { mutableStateOf(false) }
    // The widget or the notification asked for Now Playing.
    val openRequested by UiRequests.openNowPlaying.collectAsStateWithLifecycle()
    LaunchedEffect(openRequested) {
        if (openRequested) {
            nowPlayingOpen = true
            UiRequests.openNowPlaying.value = false
        }
    }
    val snackbar = remember { SnackbarHostState() }
    var bottomChrome by remember { mutableIntStateOf(0) }
    val palette = Palette.current
    val playing = rememberPlayingIds(player)
    // Taps on the Search tab, so its field takes focus (and the keyboard opens) each time.
    var searchTaps by rememberSaveable { mutableIntStateOf(0) }
    // The open long-press sheet survives rotation: its subject is saved, the callbacks rebuilt.
    var action by rememberSaveable(stateSaver = ActionRequestSaver) { mutableStateOf<ActionRequest?>(null) }
    val actionScope = rememberCoroutineScope()
    val net by NetworkMonitor.state.collectAsStateWithLifecycle()
    val stored by Downloads.stored.collectAsStateWithLifecycle()
    val cached by Availability.cached.collectAsStateWithLifecycle()
    val offlineView = remember(net.online, stored.complete, cached) { OfflineView(net.online, stored.complete, cached) }
    // The app is opening and about to use the network: what waits in the outbox goes, and pinned
    // playlists are checked against the server (once, here).
    LaunchedEffect(Unit) {
        if (NetworkMonitor.online) {
            Sync.flushIfDue("app opened")
            Downloads.syncPlaylists()
        }
    }

    val lifecycle = LocalLifecycleOwner.current.lifecycle
    LaunchedEffect(lifecycle) {
        lifecycle.repeatOnLifecycle(Lifecycle.State.STARTED) {
            Library.messages.collect { message ->
                snackbar.currentSnackbarData?.dismiss()
                snackbar.showSnackbar(message)
            }
        }
    }

    val backStack by nav.currentBackStack.collectAsStateWithLifecycle()
    val selected = backStack.asReversed().firstNotNullOfOrNull { e -> Tab.entries.firstOrNull { e.destination.hasRoute(it.routeClass) } } ?: Tab.Home
    val openTab = { tab: Tab ->
        if (tab == Tab.Search) searchTaps++
        if (tab == selected) {
            nav.popBackStack(tab.route, inclusive = false)
        } else {
            nav.navigate(tab.route) {
                popUpTo(nav.graph.findStartDestination().id) { saveState = true }
                launchSingleTop = true
                restoreState = true
            }
        }
        Unit
    }
    val top = WindowInsets.safeDrawing.only(WindowInsetsSides.Top).asPaddingValues().calculateTopPadding()
    val showMini = np.hasItem

    BoxWithConstraints(Modifier.fillMaxSize().background(palette.bg)) {
        val wide = maxWidth >= 600.dp
        CompositionLocalProvider(
            LocalScreenTop provides top,
            LocalWide provides wide,
            LocalPlaying provides playing,
            LocalItemActions provides { action = it },
            LocalOffline provides offlineView,
        ) {
            // The bar and the mini player already cover the bottom of the screen: screens that pad for
            // the keyboard (imePadding, as Search does) only pad by what the keyboard covers beyond them.
            val chromeDp = with(LocalDensity.current) { bottomChrome.toDp() }
            val content = @Composable {
                Box(Modifier.fillMaxSize().consumeWindowInsets(PaddingValues(bottom = chromeDp))) {
                    Graph(nav, session, onSignOut, searchTaps)
                    // A scrim under the status bar, so its icons stay legible over scrolled text and art:
                    // opaque behind the icons (nothing ghosts under the clock), fading out just below them.
                    Box(
                        Modifier.fillMaxWidth().height(top + 8.dp)
                            .background(
                                Brush.verticalGradient(
                                    0f to palette.bg,
                                    0.6f to palette.bg,
                                    0.8f to palette.bg.copy(alpha = 0.8f),
                                    1f to palette.bg.copy(alpha = 0f),
                                ),
                            ),
                    )
                }
            }
            val mini = @Composable { modifier: Modifier ->
                if (showMini) {
                    MiniPlayer(
                        np,
                        onTogglePlay = player::togglePlay,
                        onPrevious = { player.previous() },
                        onNext = { player.next() },
                        onOpen = { nowPlayingOpen = true },
                        modifier = modifier,
                        active = !nowPlayingOpen,
                    )
                }
            }
            // One layout for both widths, so the content (the NavHost and everything it saved) keeps its
            // place in the composition when a rotation swaps the bottom bar for the rail: slot 1 is
            // always the content, slot 2 the rail (empty when compact), slot 3 the bottom chrome.
            Layout(
                contents = listOf(
                    {
                        val sides = if (wide) WindowInsetsSides.End else WindowInsetsSides.Horizontal
                        Box(Modifier.windowInsetsPadding(WindowInsets.safeDrawing.only(sides))) { content() }
                    },
                    {
                        if (wide) {
                            NavigationRail(
                                containerColor = palette.surface,
                                windowInsets = WindowInsets.safeDrawing.only(WindowInsetsSides.Start + WindowInsetsSides.Vertical),
                            ) {
                                Spacer(Modifier.height(8.dp))
                                for (tab in Tab.entries) {
                                    NavigationRailItem(
                                        selected = tab == selected,
                                        onClick = { openTab(tab) },
                                        icon = { Icon(tab.icon, contentDescription = null) },
                                        label = { Text(tab.label) },
                                        colors = NavigationRailItemDefaults.colors(
                                            selectedIconColor = palette.accent,
                                            selectedTextColor = palette.accent,
                                            indicatorColor = palette.accent.copy(alpha = 0.16f),
                                            unselectedIconColor = palette.textDim,
                                            unselectedTextColor = palette.textDim,
                                        ),
                                    )
                                }
                            }
                        }
                    },
                    {
                        val sides = if (wide) WindowInsetsSides.End else WindowInsetsSides.Horizontal
                        Column(Modifier.onSizeChanged { bottomChrome = it.height }) {
                            OfflineBanner(!net.online, Modifier.windowInsetsPadding(WindowInsets.safeDrawing.only(sides)))
                            mini(Modifier.windowInsetsPadding(WindowInsets.safeDrawing.only(sides)))
                            if (wide) {
                                // The system bars only, not the keyboard (safeDrawing has it): as the bottom
                                // bar does, the mini player stays behind the keyboard instead of riding up on
                                // it, which in landscape left Search no room for its results.
                                Spacer(Modifier.windowInsetsBottomHeight(WindowInsets.systemBars.union(WindowInsets.displayCutout)))
                            } else {
                                NavigationBar(containerColor = palette.surface) {
                                    for (tab in Tab.entries) {
                                        NavigationBarItem(
                                            selected = tab == selected,
                                            onClick = { openTab(tab) },
                                            icon = { Icon(tab.icon, contentDescription = null) },
                                            label = { Text(tab.label) },
                                            colors = NavigationBarItemDefaults.colors(
                                                selectedIconColor = palette.accent,
                                                selectedTextColor = palette.accent,
                                                indicatorColor = palette.accent.copy(alpha = 0.16f),
                                                unselectedIconColor = palette.textDim,
                                                unselectedTextColor = palette.textDim,
                                            ),
                                        )
                                    }
                                }
                            }
                        }
                    },
                ),
                // Hidden from TalkBack while Now Playing covers it: the screen under it, the mini player
                // and the bar stayed in the accessibility tree, so swiping reached controls out of sight.
                modifier = Modifier.fillMaxSize().then(if (nowPlayingOpen && showMini) Modifier.clearAndSetSemantics { } else Modifier),
            ) { (contentM, railM, bottomM), constraints ->
                val w = constraints.maxWidth
                val h = constraints.maxHeight
                val rail = railM.map { it.measure(Constraints(maxWidth = w, minHeight = h, maxHeight = h)) }
                val railW = rail.maxOfOrNull { it.width } ?: 0
                val bottom = bottomM.map { it.measure(Constraints(minWidth = w - railW, maxWidth = w - railW, maxHeight = h)) }
                val bottomH = bottom.sumOf { it.height }
                val contentH = (h - bottomH).coerceAtLeast(0)
                val main = contentM.map { it.measure(Constraints.fixed(w - railW, contentH)) }
                layout(w, h) {
                    rail.forEach { it.place(0, 0) }
                    main.forEach { it.place(railW, 0) }
                    var y = contentH
                    bottom.forEach { it.place(railW, y); y += it.height }
                }
            }

            AnimatedVisibility(
                visible = nowPlayingOpen && showMini,
                enter = slideInVertically(tween(280)) { it },
                exit = slideOutVertically(tween(220)) { it },
            ) {
                NowPlayingScreen(
                    np = np,
                    queue = queue,
                    player = player,
                    onClose = { nowPlayingOpen = false },
                    onOpenAlbum = { nowPlayingOpen = false; nav.navigate(AlbumRoute(it)) },
                    onOpenArtist = { nowPlayingOpen = false; nav.navigate(ArtistRoute(it)) },
                    onOpenEqualizer = { nowPlayingOpen = false; nav.navigate(EqualizerRoute) { launchSingleTop = true } },
                    snackbar = snackbar,
                )
            }

            action?.let { request ->
                ActionSheet(
                    request = remember(request) { request.withQueueCallbacks(player) },
                    player = player,
                    playingSongId = playing.value.songId,
                    nav = ActionNavigation(
                        openAlbum = { nav.navigate(AlbumRoute(it)) },
                        openArtist = { nav.navigate(ArtistRoute(it)) },
                        beforeNavigate = { nowPlayingOpen = false },
                    ),
                    scope = actionScope,
                    onDismiss = { action = null },
                )
            }

            // Above the mini player and the bar; Now Playing shows its own, above its controls.
            if (!(nowPlayingOpen && showMini)) {
                val bottom = with(LocalDensity.current) { bottomChrome.toDp() }
                ThemedSnackbarHost(
                    snackbar,
                    Modifier
                        .align(Alignment.BottomCenter)
                        .windowInsetsPadding(WindowInsets.safeDrawing.only(WindowInsetsSides.Horizontal))
                        .padding(bottom = bottom),
                )
            }
        }
    }
}

@Composable
private fun Graph(nav: NavHostController, session: SessionState.SignedIn, onSignOut: () -> Unit, searchTaps: Int) {
    val back = { nav.navigateUp(); Unit }
    val openAlbum = { id: String -> nav.navigate(AlbumRoute(id)) }
    val openArtist = { id: String -> nav.navigate(ArtistRoute(id)) }
    val openPlaylist = { id: String -> nav.navigate(PlaylistRoute(id)) }
    NavHost(
        navController = nav,
        startDestination = HomeRoute,
        enterTransition = { fadeIn(tween(180)) },
        exitTransition = { fadeOut(tween(180)) },
        popEnterTransition = { fadeIn(tween(180)) },
        popExitTransition = { fadeOut(tween(180)) },
    ) {
        composable<HomeRoute> {
            HomeScreen(
                session = session,
                onOpenAlbum = openAlbum,
                onOpenArtist = openArtist,
                onSeeAll = { nav.navigate(ShelfRoute(it.key)) },
                onSettings = { nav.navigate(SettingsRoute) },
            )
        }
        composable<LibraryRoute> {
            LibraryScreen(
                onOpenAlbum = openAlbum,
                onOpenArtist = openArtist,
                onOpenPlaylist = openPlaylist,
                onOpenGenre = { g -> nav.navigate(GenreRoute(g.value, g.albumCount, g.songCount)) },
                onOpenFavouriteSongs = { nav.navigate(FavouriteSongsRoute) },
                onOpenDownloads = { nav.navigate(DownloadsRoute) },
            )
        }
        composable<SearchRoute> {
            SearchScreen(
                focusRequest = searchTaps,
                onOpenAlbum = openAlbum,
                onOpenArtist = openArtist,
                onSeeAll = { q, kind -> nav.navigate(SearchAllRoute(q, kind)) },
            )
        }
        composable<PlaylistRoute> { entry -> PlaylistScreen(entry.toRoute<PlaylistRoute>().id, onBack = back) }
        composable<GenreRoute> { entry ->
            val r = entry.toRoute<GenreRoute>()
            GenreScreen(r.name, r.albumCount, r.songCount, onBack = back, onOpenAlbum = openAlbum)
        }
        composable<FavouriteSongsRoute> { FavouriteSongsScreen(onBack = back) }
        composable<SearchAllRoute> { entry ->
            val r = entry.toRoute<SearchAllRoute>()
            SearchAllScreen(r.query, r.kind, onBack = back, onOpenAlbum = openAlbum, onOpenArtist = openArtist)
        }
        composable<SettingsRoute> {
            SettingsScreen(
                session,
                onBack = back,
                onSignOut = onSignOut,
                onOpenDownloads = { nav.navigate(DownloadsRoute) },
                onOpenEqualizer = { nav.navigate(EqualizerRoute) },
                onOpenShelves = { nav.navigate(ShelvesRoute) },
                onOpenLicences = { nav.navigate(LicencesRoute) },
            )
        }
        composable<ShelvesRoute> { ShelvesScreen(onBack = back) }
        composable<LicencesRoute> { LicencesScreen(onBack = back) }
        composable<EqualizerRoute> { EqualizerScreen(onBack = back) }
        composable<DownloadsRoute> { DownloadsScreen(onBack = back, onOpenAlbum = openAlbum, onOpenPlaylist = openPlaylist) }
        composable<AlbumRoute> { entry -> AlbumScreen(entry.toRoute<AlbumRoute>().id, onBack = back, onOpenArtist = openArtist) }
        composable<ArtistRoute> { entry -> ArtistScreen(entry.toRoute<ArtistRoute>().id, onBack = back, onOpenAlbum = openAlbum) }
        composable<ShelfRoute> { entry ->
            val shelf = ShelfId.fromKey(entry.toRoute<ShelfRoute>().shelf) ?: ShelfId.RecentlyAdded
            ShelfGridScreen(shelf, onBack = back, onOpenAlbum = openAlbum)
        }
    }
}

/** A quiet line above the mini player while there is no usable network. */
@Composable
private fun OfflineBanner(visible: Boolean, modifier: Modifier = Modifier) {
    val palette = Palette.current
    AnimatedVisibility(visible, modifier = Modifier.background(palette.surface), enter = fadeIn(tween(200)), exit = fadeOut(tween(200))) {
        Row(
            modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 6.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Icon(Icons.Rounded.CloudOff, contentDescription = null, tint = palette.textDim, modifier = Modifier.size(16.dp))
            Text(
                "Offline · downloads and cached songs play",
                style = MaterialTheme.typography.labelMedium,
                color = palette.textDim,
            )
        }
    }
}
