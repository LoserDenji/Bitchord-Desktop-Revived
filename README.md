<div align="center">
  <h1>BitChord</h1>
  <p><strong>A fast, native YouTube Music desktop app — rebuilt in Rust.</strong></p>
  <p>Built with egui and rodio. No Electron, no embedded browser player, and light on memory while playing.</p>

  <p>
    <a href="https://github.com/LoserDenji/Bitchord-Desktop-Revived/releases/latest"><img src="https://img.shields.io/github/v/release/LoserDenji/Bitchord-Desktop-Revived?style=flat-square&color=a855f7&labelColor=0d1117" alt="Latest release"></a>
    <a href="https://github.com/LoserDenji/Bitchord-Desktop-Revived/releases"><img src="https://img.shields.io/github/downloads/LoserDenji/Bitchord-Desktop-Revived/total?style=flat-square&color=a855f7&labelColor=0d1117" alt="Downloads"></a>
    <img src="https://img.shields.io/badge/platform-Windows%20%7C%20macOS%20%7C%20Linux-333?style=flat-square&labelColor=0d1117" alt="Platforms">
    <a href="LICENSE"><img src="https://img.shields.io/github/license/LoserDenji/Bitchord-Desktop-Revived?style=flat-square&color=333&labelColor=0d1117" alt="License: GPL-3.0"></a>
  </p>

  <p>
    <a href="https://github.com/LoserDenji/Bitchord-Desktop-Revived/releases/latest">Download</a> ·
    <a href="#features">Features</a> ·
    <a href="#installation">Installation</a> ·
    <a href="#building-from-source">Build from source</a>
  </p>
</div>

> BitChord is an independent, unofficial client. It is not affiliated with, endorsed by, or connected to YouTube, Google, or Apple.

## Screenshots (thx [unwrten]([github.com/](https://github.com/unwrten)))

<table>
  <tr>
    <td width="50%"><img src="https://cdn.discordapp.com/attachments/1565450418471833781/1569333528259203203/Screenshot_2026-11-09_at_18.20.57.png?ex=6af901fd&is=6af7b07d&hm=42d0ea37750dd142aa2ce2c95beba1d7673637db09a617bd2360f23b5eb1576d&" alt="Full player with artwork, lyrics and controls"></td>
    <td width="50%"><img src="https://cdn.discordapp.com/attachments/1565450418471833781/1569333526027964567/Screenshot_2026-11-09_at_18.19.42.png?ex=6af901fd&is=6af7b07d&hm=8adf5beb5d230d352acf02ca7752c9a9077be2841eac78fd7290245b49b5d9f2&" alt="Browse view with shelves and playback controls"></td>
  </tr>
  <tr>
    <td align="center"><b>Player</b> — artwork, lyrics, queue and playback in one view</td>
    <td align="center"><b>Browse</b> — your music, shelves and navigation</td>
  </tr>
  <tr>
    <td colspan="2"><img src="https://media.discordapp.net/attachments/1565450418471833781/1569333526925672529/Screenshot_2026-11-09_at_18.20.25.png?ex=6af901fd&is=6af7b07d&hm=b2b330b95238ee21839aa46ba3c4730031bf27cd9dfb9c4e756046805241f6b1" alt="BitChord interface and playback" width="100%"></td>
  </tr>
  <tr>
    <td colspan="2" align="center"><b>Desktop experience</b> — native UI, fast playback, and quality controls</td>
  </tr>
</table>

## Features

**Playback**
- **YouTube Music, natively** — search, browse, albums, playlists, artist pages, radio, mixes, charts and your library.
- **Lossless / Hi-Res Lossless / Dolby Atmos** — play higher-quality copies from configured hi-res addon sources when a matching copy is available, with YouTube audio as the fallback.
- **Quality controls in the player** — see the active audio quality, prefer Dolby Atmos when available, and revert or upgrade quality from the player menu.
- **Automix and crossfade** — DJ-style transitions between songs, with a mixing state shown in the player during transitions.
- **Queue and playback controls** — shuffle, repeat, autoplay, song radio, next-track prefetch, volume normalization, and output device selection.
- **Downloads** — save songs for offline playback.

**Lyrics and interface**
- **Synced lyrics** — timed lyrics with provider fallbacks, timing controls, romanization and translation options.
- **Native Rust desktop UI** — a fast egui/rodio interface with system media controls, tray integration, and keyboard-friendly navigation.
- **Discord Rich Presence** — show what you’re listening to on Discord.
- **Last.fm scrobbling** — connect your Last.fm account in Settings.

**Accounts and library**
- **Google sign-in** — unlock your YouTube Music feed, likes, playlists, history and recommendations.
- **Library and uploads** — access saved music and uploaded tracks where available.
- **Listen Together / jams** — start or join a shared listening session.
- **Spotify transfer** — bring playlists and liked music over from Spotify.

## Installation

Download the latest build from [Releases](https://github.com/LoserDenji/Bitchord-Desktop-Revived/releases/latest).

- **Windows** — download the Windows release asset (`-windows-x64-setup.exe` or `-windows-x64-portable.zip`), extract/install it, and run `bitchord.exe`.

  If Windows SmartScreen appears for an unsigned build, choose **More info → Run anyway**.
  
- **macOS** — download `BitChord-<version>-macos-universal.dmg` (one build for both Apple Silicon and Intel), open it and drag BitChord into Applications.

  The build is unsigned, so the first launch needs one extra step: right-click BitChord → **Open**, or allow it under System Settings → Privacy & Security → **Open Anyway**.
  
<div align="center">
  <video src="https://github.com/user-attachments/assets/8d1dba0a-c2f7-45e4-991d-602cc7095af8" width="100%" controls>
  </video>
  <p><em>macOS installation walkthrough</em></p>
</div>

- **Linux** — install the `.deb` (Debian, Ubuntu, Mint and friends), or download the other tarball, extract it and run `./install.sh` (installs into your user directory, with a menu entry and a `bitchord` command; it offers to install any missing libraries). For a run-anywhere copy, use the `-portable-` tarball. The installed and portable copies update themselves; a `.deb` install tells you when a new version is out, to install over it.

## Building from source

You need the [Rust toolchain](https://rustup.rs/).

```sh
git clone https://github.com/LoserDenji/Bitchord-Desktop-Revived.git
cd BitChord-PC-Continued
cargo build --release
```

The built app will be in `target/release/`.

Platform builds may require native dependencies (audio/system libraries and toolchain components used by sign-in or platform integrations). Check the repository’s platform docs and release workflow for the exact prerequisites.

## Logs

The app writes `bitchord.log` beside its settings, keeping the previous run as `bitchord.prev.log` — attach both (secrets removed) to a bug report. Panics, warnings and errors are captured in it on Windows, macOS and Linux. Find it at:

- **Windows** — `%APPDATA%\ytm-rs\data\bitchord.log` (portable builds: a `data` folder next to the exe)
- **macOS** — `~/Library/Application Support/ytm-rs/data/bitchord.log`
- **Linux** — `~/.local/share/ytm-rs/data/bitchord.log`

The sign-in windows are separate processes and log to `bitchord-login.log`, `bitchord-spotify.log` and `bitchord-turnstile.log` in the same folder.

Two launch flags help when the tray icon misbehaves (taskbar replacements can keep it from ever showing):

- `bitchord --show` — bring a running copy's window back up (a plain launch does this too).
- `bitchord --no-tray` — start a session where × quits for real instead of hiding to the tray.

## Notes

- Audio and the main interface are native (Rust + egui + rodio). Sign-in and some web flows may open a system webview window.
- Hi-res/Lossless/Atmos playback depends on configured addon sources and whether a matching, playable copy exists for the selected track.
- BitChord Desktop Revived does not bypass subscriptions, DRM, or regional restrictions. Availability depends on the services and sources you use.
- Third-party services (YouTube Music, lyrics providers, Discord, Last.fm, Spotify, addon sources) can change behavior over time.

## Contributing

Bug reports and pull requests are welcome. Please open an issue with clear reproduction steps, platform details, and relevant logs (with secrets/tokens removed).

## License

BitChord Desktop Revived is licensed under [GPL-3.0](LICENSE).

## Star History

<a href="https://www.star-history.com/?repos=loserdenji%2Fbitchord-desktop-revived&type=date&legend=top-left">
 <picture>
   <source media="(prefers-color-scheme: dark)" srcset="https://api.star-history.com/chart?repos=loserdenji/bitchord-desktop-revived&type=date&theme=dark&legend=top-left" />
   <source media="(prefers-color-scheme: light)" srcset="https://api.star-history.com/chart?repos=loserdenji/bitchord-desktop-revived&type=date&legend=top-left" />
   <img alt="Star History Chart" src="https://api.star-history.com/chart?repos=loserdenji/bitchord-desktop-revived&type=date&legend=top-left" />
 </picture>
</a>

## Acknowledgements

- [BitChord](https://github.com/kushagrasinghx/BitChord) for the the orignal android app
- [ytm-rs](https://github.com/grapedjuice/ytm-rs) for the base of the app which was later modified by jackychan4891
- [jackychan4891](https://github.com/jackychan4891/BitChord-PC-Continued) for the modification in ytm-rs app for parity with BitChord
- [egui](https://github.com/emilk/egui) for the interface.
- [rodio](https://github.com/RustAudio/rodio) and Symphonia for audio playback and decoding.
- Lyrics, metadata, playback, and integrations depend on the upstream libraries and third-party services used by this project.
