package com.music.bitchord.desktop

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.foundation.interaction.MutableInteractionSource
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Window
import androidx.compose.ui.window.rememberScrollState
import androidx.compose.ui.window.rememberWindowState
import com.music.bitchord.data.model.Song
import com.music.bitchord.ui.icons.BitChordIcons
import java.awt.Toolkit
import kotlin.math.roundToInt

/**
 * The floating overlay player window.
 *
 * A small, always-on-top, frameless window that shows the current track info
 * with transport controls. Positioned along the top edge of the screen.
 */
@Composable
fun FloatingPlayerWindow(
    currentSong: Song?,
    isPlaying: Boolean,
    positionMs: Long,
    durationMs: Long,
    onPrevious: () -> Unit,
    onPlayPause: () -> Unit,
    onNext: () -> Unit,
    onToggleLike: (Song) -> Unit,
) {
    val enabled = DesktopFloatingPlayerSettings.enabled.collectAsState().value
    if (!enabled) return

    val position = DesktopFloatingPlayerSettings.position.collectAsState().value
    val windowState = rememberWindowState(
        width = 320.dp,
        height = 56.dp,
    )

    Window(
        onCloseRequest = { /* nothing — pill has no close button */ },
        visible = enabled,
        title = "",
        undecorated = true,
        transparent = true,
        alwaysOnTop = true,
        state = windowState,
    ) {
        val window = window
        val scale = window.graphicsConfiguration.defaultTransform.scaleX

        // Set AWT peer properties for transparency and always-on-top
        LaunchedEffect(window) {
            if (DesktopPlatform.isMac) {
                window.type = java.awt.Window.Type.UTILITY
            } else {
                window.type = java.awt.Window.Type.TRANSPARENT
            }
            window.setBackground(java.awt.Color(0, 0, 0, 0))
            window.isAlwaysOnTop = true
        }

        // Position the window
        LaunchedEffect(position, windowState) {
            val screenWidth = Toolkit.getDefaultToolkit().screenSize.width
            val windowWidth = (320 * scale).roundToInt()
            val x = when (position) {
                FloatingPlayerPosition.TOP_LEFT -> 0
                FloatingPlayerPosition.TOP_CENTER -> (screenWidth / 2 - windowWidth / 2).coerceAtLeast(0)
                FloatingPlayerPosition.TOP_RIGHT -> (screenWidth - windowWidth).coerceAtLeast(0)
            }
            java.awt.EventQueue.invokeLater {
                window.setLocation(x, 0)
            }
        }

        FloatingPlayerPill(
            song = currentSong,
            isPlaying = isPlaying,
            positionMs = positionMs,
            durationMs = durationMs,
            onPrevious = onPrevious,
            onPlayPause = onPlayPause,
            onNext = onNext,
            onToggleLike = onToggleLike,
        )
    }
}

/**
 * The pill-shaped content of the floating player.
 */
@Composable
private fun FloatingPlayerPill(
    song: Song?,
    isPlaying: Boolean,
    positionMs: Long,
    durationMs: Long,
    onPrevious: () -> Unit,
    onPlayPause: () -> Unit,
    onNext: () -> Unit,
    onToggleLike: (Song) -> Unit,
) {
    // Calculate progress fraction with smooth animation
    var progressFraction by remember { mutableStateOf(0f) }

    LaunchedEffect(positionMs, durationMs) {
        if (durationMs > 0) {
            progressFraction = positionMs.coerceAtLeast(0L).toFloat() / durationMs
        } else {
            progressFraction = 0f
        }
    }

    if (song == null) {
        // Empty state - show placeholder
        Box(
            modifier = Modifier
                .fillMaxSize()
                .clip(RoundedCornerShape(28.dp))
                .background(Color(0xFF1F1F1F), RoundedCornerShape(28.dp))
                .padding(horizontal = 12.dp, vertical = 8.dp),
            contentAlignment = Alignment.Center
        ) {
            Text(
                text = "",
                style = MaterialTheme.typography.bodySmall,
                color = Color.Gray
            )
        }
        return
    }

    Box(
        modifier = Modifier
            .fillMaxSize()
            .clip(RoundedCornerShape(28.dp))
    ) {
        // Background
        Box(
            modifier = Modifier
                .fillMaxSize()
                .background(
                    color = Color(0xFF1F1F1F),
                    shape = RoundedCornerShape(28.dp)
                )
                .clickable(
                    interactionSource = remember { MutableInteractionSource() },
                    indication = null,
                    modifier = Modifier
                )
        )

        // Content
        Row(
            modifier = Modifier
                .fillMaxSize()
                .padding(horizontal = 12.dp, vertical = 8.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp)
        ) {
            // Artwork
            DesktopArtwork(
                url = song.thumbnailUrl,
                modifier = Modifier.size(32.dp),
                contentScale = ContentScale.Crop,
                contentDescription = song.title,
            )

            // Song info with scrolling title
            Column(
                modifier = Modifier
                    .weight(1f)
                    .horizontalScroll(rememberScrollState()),
                horizontalAlignment = Alignment.CenterHorizontally
            ) {
                Text(
                    text = song.title,
                    style = MaterialTheme.typography.bodySmall,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis
                )
                Text(
                    text = song.artist,
                    style = MaterialTheme.typography.bodySmall,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    color = Color.Gray
                )
            }

            // Like button
            IconButton(onClick = { onToggleLike(song) }) {
                val isLiked = song.videoId?.let { DesktopPersistence().likedIds().contains(it) } ?: false
                Icon(
                    imageVector = if (isLiked) BitChordIcons.StarFilled else BitChordIcons.Star,
                    contentDescription = "Like",
                    tint = if (isLiked) Color(0xFFFFD700) else Color.White,
                    modifier = Modifier.size(18.dp)
                )
            }

            Spacer(Modifier.width(4.dp))

            // Previous button
            IconButton(onClick = onPrevious) {
                Icon(
                    imageVector = BitChordIcons.TransportPrevious,
                    contentDescription = "Previous",
                    tint = Color.White,
                    modifier = Modifier.size(18.dp)
                )
            }

            // Play/Pause button
            IconButton(onClick = onPlayPause) {
                Icon(
                    imageVector = if (isPlaying) BitChordIcons.TransportPause else BitChordIcons.TransportPlay,
                    contentDescription = if (isPlaying) "Pause" else "Play",
                    tint = Color.White,
                    modifier = Modifier.size(18.dp)
                )
            }

            // Next button
            IconButton(onClick = onNext) {
                Icon(
                    imageVector = BitChordIcons.TransportNext,
                    contentDescription = "Next",
                    tint = Color.White,
                    modifier = Modifier.size(18.dp)
                )
            }
        }

        // Progress bar at the bottom
        LinearProgressIndicator(
            progress = { progressFraction },
            modifier = Modifier
                .fillMaxWidth()
                .align(Alignment.BottomCenter)
                .height(3.dp),
            color = Color(0xFFFA586A),
            trackColor = Color.White.copy(alpha = 0.15f)
        )
    }
}