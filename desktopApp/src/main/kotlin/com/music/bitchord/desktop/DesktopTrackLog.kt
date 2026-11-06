package com.music.bitchord.desktop

import java.io.BufferedWriter
import java.nio.file.Files
import java.nio.file.Path
import java.nio.file.StandardOpenOption
import java.time.LocalTime
import java.time.format.DateTimeFormatter

/** The few lines a resolve has to say about itself. */
internal object DesktopTrackLog {

    private val clock = DateTimeFormatter.ofPattern("HH:mm:ss")

    /** `$XDG_STATE_HOME/bitchord/desktop.log`, or the usual default under $HOME. */
    private val file: Path? by lazy {
        runCatching {
            val state = System.getenv("XDG_STATE_HOME")?.takeIf { it.isNotBlank() }
                ?: "${System.getProperty("user.home")}/.local/state"
            val directory = Path.of(state, "bitchord")
            Files.createDirectories(directory)
            directory.resolve("desktop.log")
        }.getOrNull()
    }

    fun log(line: String) {
        val stamped = "${LocalTime.now().format(clock)}  $line"
        println("BitChord: $stamped")
        val target = file ?: return
        runCatching {
            // One writer held open and flushed per line, rather than an open, a
            // write and a close of the file per line on the caller's thread — the
            // shared data layer logs through here, so that was three syscalls a
            // line on whatever thread happened to be resolving or seeking.
            (writer ?: synchronized(this) {
                writer ?: Files.newBufferedWriter(
                    target,
                    StandardOpenOption.CREATE,
                    StandardOpenOption.APPEND,
                ).also { writer = it }
            }).let { out ->
                synchronized(out) {
                    out.write(stamped)
                    out.newLine()
                    out.flush()
                }
            }
        }.onFailure { runCatching { writer?.close() }; writer = null }
    }

    @Volatile
    private var writer: BufferedWriter? = null
}
