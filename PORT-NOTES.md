# Port notes — the Dynamic Island (from Airbeats-Desktop)

The floating now-playing card from [d0x-dev/Airbeats-Desktop](https://github.com/d0x-dev/Airbeats-Desktop)
(`electron-app/dynamic-island.*`, Electron) was ported to BitChord's Tauri shell
and re-dressed so it reads as BitChord, not Airbeats: the app's own frosted-glass
recipe, DM Sans, the Apple-Music key colour, and the shell's existing control
path. Airbeats' feature set is preserved — a mini pill with artwork and a
remaining-time label, a full card with scrubber and transport, auto-expand on a
new track, auto-collapse after five seconds, hidden while paused — minus its
like/repeat buttons, which BitChord's media session does not expose (the island
offers exactly the controls the tray already had, plus seeking).

## How it works

```
main webview (the web frontend, untouched)
  └─ desktop.js shim ── media_update ──▶ media::apply_update (the snapshot)
                                          ├─ tray Play/Pause label
                                          ├─ MPRIS / Now Playing
                                          └─ island::sync ── eval ──▶ island webview
island webview (island.html + src/island/)
  └─ presses ── island_action ──▶ media::eval_in_main("window.__BitChordMediaAction(…)")
                                   (the same buttons the tray presses)
```

- **State in.** The Rust shell already receives every metadata / playback /
  position update in one place (`media::apply_update`). The island is a fourth
  mirror of that snapshot, eval'd in as `window.__IslandSync({...})` — the same
  shape as the core's `__BitChordBridgeEvent` push. The island interpolates the
  position between syncs (`position + rate·Δt`, drawn ten times a second like
  the app's own `useScrubber`), so the throttled (~900 ms) updates look smooth.
- **Presses out.** Transport goes through `island_action` to
  `window.__BitChordMediaAction` — identical to the tray's menu items, so play/
  pause/next/previous/seek behave exactly as the hardware media keys do.
  `open` raises the main window (shared with the tray's Open).
- **Window management.** The island is a second Tauri window (`island`):
  frameless, transparent, always-on-top, skip-taskbar, no shadow, never focused
  at launch, pinned top-centre of the current monitor (below the menu bar on
  macOS). Because a transparent window still eats clicks over its invisible
  areas, the OS window is resized around the animated card — grown before the
  expand animation, shrunk ~440 ms after the collapse — and re-centred each
  time. While hidden it hands its rectangle back with
  `set_ignore_cursor_events(true)`.
- **Toggle.** The settings page's **Desktop → Dynamic Island** switch, served
  over the bridge (`island.state` / `island.setEnabled`, capability `island`)
  and persisted in the core store (`island_enabled`, `None` = on, the shipping
  default). It used to be a tray checkbox; the tray item is gone — Settings is
  where every other feature is chosen. The window-state plugin must never
  track the island (denylisted like the agent windows): its geometry is
  computed, not restored.

## Design — why it belongs here

Every value in `app/src/island/island.css` is traced to a token in
`app/src/styles/tokens.css` (dark scheme) or a recipe in `global.css`:

| Island | Source |
| --- | --- |
| fill `#26262899`, blur `saturate(2.2) blur(16px)` | `--glassMaterialBackground`, `--glassMaterialBackdropFilter` — the `glass-pill` recipe |
| inner stroke 0.5px white @ .25 | `--glassMaterialInnerStroke(+Alpha)`, dark |
| text `#ffffffeb / #ffffffa3 / #ffffff66` | `--systemPrimary/Secondary/Tertiary-onDark` |
| key `#fa586a` | `--keyColor`, dark |
| type: 600 13px titles, 500 12px artist, 600 11px tabular times | `type-body-emphasized`, `type-callout-medium`, the LCD scrubber's times |
| radius 1000px mini · 40px expanded; artwork 5px / 6px | `--radius-pill`, artwork tokens and the LCD tile; the expanded 40px is the reference island's own `.expanded` radius (a stadium curve would clip the artwork's corner) |
| ease `cubic-bezier(0.215,0.61,0.355,1)` | `--ease-out-panel` |
| opaque `#0e0e0e` under `prefers-contrast: more` | the increased-contrast glass rule |

The icons are the player pill's own MusicKit transport glyphs (copied from
`components/icons/Transport.tsx`), the artwork placeholder is the LCD's
(`--genericJoeColor` + the logo mark), and artwork URLs go through the app's
`sizedArtwork()` so the CDN sees the same cached sizes. The island always takes
the on-dark colours, the same rule the full-screen player follows. DM Sans is
loaded with the exact request `index.html` uses.

## Files

Added:

- `src-tauri/src/island.rs` — the window: create/destroy, toggle + persistence,
  `sync`, `apply_size`, `set_click_through`, `handle_action`
- `src-tauri/shim/island.js` — init script exposing `window.__IslandHost.invoke`
- `src-tauri/capabilities/island.json` — the island's three commands, nothing else
- `app/island.html`, `app/src/island/island.css`, `app/src/island/main.ts`
- this file

Changed:

- `src-tauri/core/src/store.rs` — `island_enabled: Option<bool>` (serde default
  keeps old `state.json` files valid)
- `src-tauri/src/commands.rs` — `island_action` / `island_resize` /
  `island_overlay` commands (async, like every command that touches windows)
- `src-tauri/src/lib.rs` — module, command registration, `island::init` after
  the tray, `island` added to the window-state denylist
- `src-tauri/src/media.rs` — `apply_update` mirrors the snapshot into the
  island; two long-standing warnings fixed on the Linux/macOS path while there
  (`attach()` needs `mut`, a dead `handle` clone, a doc comment on
  `thread_local!`)
- `src-tauri/src/tray.rs` — the checkable Dynamic Island item (`show_main` is
  now `pub`, shared with the island's card)
- `src-tauri/permissions/desktop-commands.toml` — the three allow-* permissions
- `src-tauri/tauri.conf.json` — `"island"` in `app.security.capabilities`
- `app/vite.config.ts` — `island.html` as a second build input
- `README.md` — the shell feature list

The web frontend itself is untouched, byte for byte — the island is a separate
page of the same build (`island.html`), which the deployed web site will simply
never link.

## Testing it

```bash
pnpm install
pnpm tauri dev      # play something; the pill appears top-centre
```

Or with the built app: click a track, watch the card expand and fold after five
seconds; click the pill to expand it again; drag the seek line; click the
artwork to bring the app up; pause and it vanishes. `Settings → Desktop →
Dynamic Island` turns the feature off and remembers.

Verified in this working tree: `pnpm typecheck` clean, `cargo check` clean
(zero warnings), `bitchord-core` tests 11/11, and the island page exercised in
a browser at both window sizes (216×44, 432×188) — mini render, auto-expand,
pause-hide, resume, artwork fallback, time interpolation.

One note for Linux packagers: transparency needs a compositor (every mainstream
desktop has one); without one the pill's glass renders opaque, which is the
degraded look, not a broken one.

## The island after the focus round (v1.9.5)

The island is no longer "up whenever music plays". It is BitChord's glance
*away* from the app: the shell mirrors the main window's focus into the
island page (`Focused` events → `__IslandFocus`, one `island_focus_state`
query at load), and a single `reconcile()` in `island/main.ts` decides the
mode from exactly three inputs — playback (paused wins: nothing), the app's
focus (frontmost: nothing), and a right-click dismissal (lasts until the
user has been back in BitChord and left again). Music playing + app in the
background is the only state where the card exists. A song change refreshes
the card's contents and never its visibility — the auto-expand of the
Airbeats port is gone — and the only way to grow it is the user's own
click, with the same five-second auto-collapse.

Arrivals from `hidden` spawn: `island-spawn` pops the card from 0.55 scale
with a settle dip, all inside the card's own box (the OS window that hugs
the pill never clips a frame of it). And the grow/shrink of the OS window
itself is now one atomic `SetWindowPos` on Windows — a plain `set_size`
anchors the top-left corner, and the pill used to read the re-centre as a
second step ("centre left, then centre"). macOS/Linux keep the two-call
path inside one main-thread task.

The top bar of the main window is the app's own page now: frameless on
Windows/Linux (`decorations(false)`; macOS keeps the overlay traffic
lights), with `shell/WindowControls.tsx` drawing the transparent strip —
window buttons top right, drag on empty top-of-window space, double-click
to maximize — and the sidebar carrying no wordmark. The bar paints nothing
and blocks nothing; it is a hint to the eye, not a surface.

## The lyric clock

`shell/LyricsPanel.tsx` no longer polls am-lyrics thirty times a second.
The TTML the panel already holds is parsed into a sorted set of instants —
line starts and ends, every word and syllable edge, background vocals — and
the clock writes only there: exactly at each boundary, at the leads
am-lyrics looks ahead from (2000 / 500 / 400 ms), and a bounded heartbeat
in between. Every visual is still am-lyrics' own (wipes, rises, blur and
scroll are self-running CSS/WAAPI animations), so the look is untouched;
what changed is that the main thread stops sweeping every word sixty times
a second for "nothing changed". Seeks and pauses still land at once, and a
TTML that will not parse falls back to the old cadence.
