package com.music.bitchord.desktop

import kotlin.test.AfterTest
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertTrue

/**
 * Tests for the floating player settings persistence.
 */
class DesktopFloatingPlayerSettingsTest {

    @AfterTest
    fun tearDown() {
        // Reset to defaults after each test
        DesktopFloatingPlayerSettings.setEnabled(false)
        DesktopFloatingPlayerSettings.setAutoShow(true)
        DesktopFloatingPlayerSettings.setPosition(FloatingPlayerPosition.TOP_RIGHT)
    }

    @Test
    fun `default enabled is false`() {
        assertFalse(DesktopFloatingPlayerSettings.enabled.value, "enabled should default to false")
    }

    @Test
    fun `default autoShow is true`() {
        assertTrue(DesktopFloatingPlayerSettings.autoShow.value, "autoShow should default to true")
    }

    @Test
    fun `default position is top right`() {
        assertEquals(FloatingPlayerPosition.TOP_RIGHT, DesktopFloatingPlayerSettings.position.value)
    }

    @Test
    fun `setEnabled persists and updates StateFlow`() {
        // Set to true
        DesktopFloatingPlayerSettings.setEnabled(true)
        assertTrue(DesktopFloatingPlayerSettings.enabled.value, "enabled should be true after setting")

        // Set to false
        DesktopFloatingPlayerSettings.setEnabled(false)
        assertFalse(DesktopFloatingPlayerSettings.enabled.value, "enabled should be false after setting")
    }

    @Test
    fun `setAutoShow persists and updates StateFlow`() {
        // Set to false
        DesktopFloatingPlayerSettings.setAutoShow(false)
        assertFalse(DesktopFloatingPlayerSettings.autoShow.value, "autoShow should be false after setting")

        // Set to true
        DesktopFloatingPlayerSettings.setAutoShow(true)
        assertTrue(DesktopFloatingPlayerSettings.autoShow.value, "autoShow should be true after setting")
    }

    @Test
    fun `setPosition persists and round-trips through enum`() {
        // Test each position
        FloatingPlayerPosition.values().forEach { position ->
            DesktopFloatingPlayerSettings.setPosition(position)
            assertEquals(position, DesktopFloatingPlayerSettings.position.value)
        }
    }
}