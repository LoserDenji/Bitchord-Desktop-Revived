package com.music.bitchord.desktop

import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow

/**
 * Where the floating player sits on screen. Three positions, all along the top edge, so the
 * pill is always reachable from wherever the main window was left.
 */
enum class FloatingPlayerPosition {
    TOP_LEFT,
    TOP_CENTER,
    TOP_RIGHT,
}

/**
 * Settings for the floating overlay player, persisted through [DesktopPersistence].
 *
 * Each field is exposed as a [StateFlow] so the UI can collect it without re-reading the
 * preference store on every recomposition. The object is a singleton: the settings screen and
 * the floating window both read from the same instance.
 */
internal object DesktopFloatingPlayerSettings {

    private const val KEY_ENABLED = "floating_player_enabled"
    private const val KEY_AUTO_SHOW = "floating_player_auto_show"
    private const val KEY_POSITION = "floating_player_position"

    private val persistence = DesktopPersistence()

    private val _enabled = MutableStateFlow(persistence.boolean(KEY_ENABLED, false))
    val enabled: StateFlow<Boolean> = _enabled

    private val _autoShow = MutableStateFlow(persistence.boolean(KEY_AUTO_SHOW, true))
    val autoShow: StateFlow<Boolean> = _autoShow

    private val _position = MutableStateFlow(
        runCatching {
            FloatingPlayerPosition.valueOf(
                persistence.string(KEY_POSITION, FloatingPlayerPosition.TOP_RIGHT.name),
            )
        }.getOrDefault(FloatingPlayerPosition.TOP_RIGHT),
    )
    val position: StateFlow<FloatingPlayerPosition> = _position

    fun setEnabled(value: Boolean) {
        _enabled.value = value
        persistence.saveBoolean(KEY_ENABLED, value)
    }

    fun setAutoShow(value: Boolean) {
        _autoShow.value = value
        persistence.saveBoolean(KEY_AUTO_SHOW, value)
    }

    fun setPosition(value: FloatingPlayerPosition) {
        _position.value = value
        persistence.saveString(KEY_POSITION, value.name)
    }
}