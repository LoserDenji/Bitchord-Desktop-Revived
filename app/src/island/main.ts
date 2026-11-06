/**
 * The Dynamic Island's brain.
 *
 * State flows in one way: the shell evals `window.__IslandSync(<snapshot>)`
 * on every media update (island.rs::sync — the same snapshot the tray and
 * MPRIS read). Presses flow out through three commands, mirroring the tray:
 * `island_action` for transport (it presses the page's own media-session
 * handlers — the island never talks to the app page directly), `island_resize`
 * to grow/shrink the OS window around the animated card, `island_overlay` to
 * hand the hidden rectangle back to the desktop.
 *
 * Behaviour is Airbeats' island (d0x-dev/Airbeats-Desktop): a new track
 * expands the card for a few seconds, a click expands it again, and pause
 * takes the pill away entirely. The drawing follows the app's own scrubber
 * (`useScrubber`): a `--progress` transform on the fill, seek on release,
 * steady playback drawn ten times a second, drags at the frame rate.
 */

import { sizedArtwork } from '@/lib/artwork';

type Snapshot = {
  state: string;
  title: string | null;
  artist: string | null;
  album: string | null;
  artworkUrl: string | null;
  duration: number;
  position: number;
  rate: number;
};

type Host = { invoke: (cmd: string, args?: Record<string, unknown>) => Promise<unknown> };

declare global {
  interface Window {
    __IslandSync: (snapshot: Snapshot) => void;
  }
}

const host: Host | null = (window as unknown as { __IslandHost?: Host }).__IslandHost ?? null;

/** Every press is best-effort: the island must survive a missing host (it
 * renders in a plain browser for design work) and a dead IPC alike. */
function invoke(cmd: string, args?: Record<string, unknown>): void {
  try {
    void host?.invoke(cmd, args);
  } catch {
    /* the overlay is cosmetic; nothing here may take the app down */
  }
}

/** The same, for the asks whose landing the page animates against: the shell
 * resolves the command once it has been handed to the main thread. */
function invokeAsync(cmd: string, args?: Record<string, unknown>): Promise<unknown> {
  try {
    return host?.invoke(cmd, args) ?? Promise.resolve();
  } catch {
    return Promise.resolve();
  }
}

const island = document.getElementById('island')!;
const miniTitle = document.getElementById('mini-title')!;
const miniRemaining = document.getElementById('mini-remaining')!;
const fullTitle = document.getElementById('full-title')!;
const fullArtist = document.getElementById('full-artist')!;
const timeElapsed = document.getElementById('time-elapsed')!;
const timeRemaining = document.getElementById('time-remaining')!;
const fill = document.getElementById('scrub-fill')!;
const scrub = document.getElementById('scrub')!;
const rail = fill.parentElement!;
const playButton = document.getElementById('btn-play')!;
const fullArt = document.getElementById('full-art')!;
const artImages = Array.from(document.querySelectorAll<HTMLImageElement>('.art img'));
const artFrames = Array.from(document.querySelectorAll<HTMLElement>('.art'));

const COLLAPSE_AFTER_MS = 5000; // Airbeats' dwell time for an expanded card
const RESIZE_SETTLE_MS = 440; // the card's own width/height transition

/** The last snapshot the shell sent. */
let snap: Snapshot | null = null;
/** performance.now() when that snapshot's position was true. */
let anchor = 0;
/** The visible state, also mirrored into the OS window's size. */
let mode: 'hidden' | 'mini' | 'full' = 'hidden';
/** The metadata the card is currently displaying, for change detection. */
let renderedKey = '';
let collapseTimer: number | null = null;
let pendingShrink: number | null = null;
/** Fraction while dragging the seek line, else null. */
let dragging: number | null = null;
let rafId = 0;
let lastDraw = -Infinity;
/** Stamps each expand request; a collapsed (or re-hidden) card leaves the
 * late-arriving window-grow with nothing to animate. */
let expandToken = 0;

function fmt(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds < 0) return '0:00';
  const total = Math.floor(seconds);
  const m = Math.floor(total / 60);
  const s = total % 60;
  return `${m}:${s.toString().padStart(2, '0')}`;
}

/** Position carried forward by the rate — the shell throttles its updates
 * (~900 ms); this is what makes the line move like the app's does. */
function livePosition(now: number): number {
  if (!snap) return 0;
  if (dragging !== null) return dragging * (snap.duration > 0 ? snap.duration : 0);
  const base = snap.position;
  if (snap.state !== 'playing') return base;
  return base + ((now - anchor) / 1000) * (snap.rate || 1);
}

function draw(now: number): void {
  if (!snap) return;
  const duration = snap.duration > 0 ? snap.duration : 0;
  const position = Math.min(Math.max(livePosition(now), 0), duration || Math.max(0, livePosition(now)));
  const progress = duration > 0 ? Math.min(1, Math.max(0, position / duration)) : 0;
  fill.style.setProperty('--progress', progress.toFixed(4));
  timeElapsed.textContent = fmt(position);
  const remaining = duration > 0 ? fmt(Math.max(0, duration - position)) : '0:00';
  timeRemaining.textContent = `-${remaining}`;
  miniRemaining.textContent = duration > 0 ? `-${remaining}` : '';
}

/** The scrubber's idle cadence: ten draws a second, frame-rate while a drag
 * holds the line, nothing at all when hidden (a hidden webview paints
 * nothing, but neither should its loop burn cycles). The mini pill draws too:
 * its remaining-time label rides the same clock. */
function tick(now: number): void {
  rafId = 0;
  if (!snap || mode === 'hidden') return;
  const due = dragging !== null || now - lastDraw >= 100;
  if (due) {
    lastDraw = now;
    draw(now);
  }
  rafId = requestAnimationFrame(tick);
}

function schedule(): void {
  if (!rafId && mode !== 'hidden') rafId = requestAnimationFrame(tick);
}

function setClasses(next: typeof mode): void {
  island.classList.toggle('hidden', next === 'hidden');
  island.classList.toggle('mini', next === 'mini');
  island.classList.toggle('full', next === 'full');
}

/** Two painted frames. The shell resolves `island_resize` when the main
 * thread has the geometry task; by the compositor's next paint that task has
 * run, so the morph never starts inside a window still clipping it. (In a
 * plain browser there is no shell and no clipping; the promise resolves at
 * once and the frames are all the wait there is.) */
function nextPaint(): Promise<void> {
  return new Promise((resolve) => {
    requestAnimationFrame(() => requestAnimationFrame(() => resolve()));
  });
}

function applyMode(next: typeof mode): void {
  if (mode === next) return;
  const previous = mode;
  mode = next;
  // The window hugs the card. Expanding grows the window first and only then
  // starts the morph — a 216px window would clip the growing card's first
  // frames and read as a stutter. Collapsing animates at once (the big window
  // has room), then shrinks once the card has settled — whatever it collapsed
  // into: a pause while expanded must hand the small window back too, or the
  // resumed pill sits in a 432px window that eats clicks around it.
  if (next === 'full') {
    if (pendingShrink !== null) {
      clearTimeout(pendingShrink);
      pendingShrink = null;
    }
    const token = ++expandToken;
    void invokeAsync('island_resize', { expanded: true })
      .then(() => nextPaint())
      .then(() => {
        if (expandToken !== token || mode !== 'full') return;
        setClasses('full');
      });
  } else {
    ++expandToken;
    setClasses(next);
    if (previous === 'full') {
      if (pendingShrink !== null) clearTimeout(pendingShrink);
      pendingShrink = window.setTimeout(() => {
        pendingShrink = null;
        invoke('island_resize', { expanded: false });
      }, RESIZE_SETTLE_MS);
    }
  }
  invoke('island_overlay', { hidden: next === 'hidden' });
  lastDraw = -Infinity;
  schedule();
}

function cancelCollapse(): void {
  if (collapseTimer !== null) {
    clearTimeout(collapseTimer);
    collapseTimer = null;
  }
}

function scheduleCollapse(): void {
  cancelCollapse();
  collapseTimer = window.setTimeout(() => {
    collapseTimer = null;
    if (mode === 'full') applyMode('mini');
  }, COLLAPSE_AFTER_MS);
}

function expand(): void {
  applyMode('full');
  scheduleCollapse();
}

function collapse(): void {
  cancelCollapse();
  if (mode === 'full') applyMode('mini');
}

/** Renders metadata; returns whether the track itself changed. */
function renderMetadata(s: Snapshot): boolean {
  const key = `${s.title ?? ''}\u0000${s.artist ?? ''}\u0000${s.artworkUrl ?? ''}`;
  const changed = key !== renderedKey && Boolean(s.title || s.artworkUrl);
  renderedKey = key;
  fullTitle.textContent = s.title ?? '';
  fullArtist.textContent = s.artist ?? '';
  miniTitle.textContent = s.title ?? '';
  fullTitle.title = s.title ?? '';
  const src = s.artworkUrl ? sizedArtwork(s.artworkUrl, 48) : '';
  for (const img of artImages) {
    if (img.getAttribute('src') !== src) {
      img.classList.remove('loaded');
      if (src) img.src = src;
      else img.removeAttribute('src');
    }
  }
  for (const frame of artFrames) frame.classList.toggle('bare', !s.artworkUrl);
  return changed;
}

function renderPlayback(s: Snapshot): void {
  const playing = s.state === 'playing';
  playButton.classList.toggle('paused', !playing);
  playButton.setAttribute('aria-label', playing ? 'Pause' : 'Play');
}

window.__IslandSync = (raw: Snapshot) => {
  const previous = snap;
  snap = raw;
  anchor = performance.now();

  const trackChanged = renderMetadata(raw);
  renderPlayback(raw);

  if (raw.state === 'playing') {
    // Hide on pause, return on play — the pill only exists while music does.
    if (mode === 'hidden') applyMode('mini');
    // Airbeats' flourish: a new track introduces itself, then folds away.
    if (trackChanged && previous !== null && previous.state === 'playing' && mode === 'mini') {
      expand();
    }
  } else if (raw.state === 'paused' || raw.state === 'stopped') {
    cancelCollapse();
    applyMode('hidden');
  }

  lastDraw = -Infinity;
  schedule();
};

// ---- The presses -----------------------------------------------------------

// The card expands on a click of the pill itself; the controls are excluded,
// like Airbeats excluded its own.
island.addEventListener('click', (event) => {
  if (mode !== 'mini') return;
  if ((event.target as HTMLElement | null)?.closest('button')) return;
  expand();
});

document.getElementById('btn-previous')!.addEventListener('click', () => {
  invoke('island_action', { action: 'previoustrack' });
  scheduleCollapse();
});

document.getElementById('btn-next')!.addEventListener('click', () => {
  invoke('island_action', { action: 'nexttrack' });
  scheduleCollapse();
});

playButton.addEventListener('click', () => {
  // Explicit play/pause, resolved locally — the same resolution the tray's
  // Play/Pause item makes, so both stay honest against the same snapshot.
  invoke('island_action', { action: snap?.state === 'playing' ? 'pause' : 'play' });
  scheduleCollapse();
});

// The artwork raises the app, as the LCD's opens the full-screen player.
fullArt.addEventListener('click', () => {
  invoke('island_action', { action: 'open' });
});

// ---- The seek line ----------------------------------------------------------

function fractionAt(event: PointerEvent): number {
  const rect = rail.getBoundingClientRect();
  if (rect.width <= 0) return 0;
  return Math.min(1, Math.max(0, (event.clientX - rect.left) / rect.width));
}

scrub.addEventListener('pointerdown', (event) => {
  if (!snap || snap.duration <= 0) return;
  dragging = fractionAt(event);
  scrub.classList.add('dragging');
  scrub.setPointerCapture(event.pointerId);
  lastDraw = -Infinity;
  schedule();
});

scrub.addEventListener('pointermove', (event) => {
  if (dragging === null) return;
  dragging = fractionAt(event);
  lastDraw = -Infinity;
  schedule();
});

const endDrag = (event: PointerEvent) => {
  if (dragging === null) return;
  const fraction = fractionAt(event);
  dragging = null;
  scrub.classList.remove('dragging');
  if (snap && snap.duration > 0) {
    invoke('island_action', { action: 'seekto', arg: fraction * snap.duration });
    // The shell's next position sync confirms it; hold the picked spot until
    // then so the line does not snap backwards.
    snap.position = fraction * snap.duration;
    anchor = performance.now();
  }
  scheduleCollapse();
};

scrub.addEventListener('pointerup', endDrag);
scrub.addEventListener('pointercancel', endDrag);

// ---- Window manners ---------------------------------------------------------

// Focus leaving the island folds it back, the way the phone's card folds when
// you touch elsewhere. The auto-collapse timer covers the no-click case.
window.addEventListener('blur', () => {
  if (dragging !== null) return;
  collapse();
});

// Artwork arrives or fails independent of the state syncs.
for (const img of artImages) {
  img.addEventListener('load', () => img.classList.add('loaded'));
  img.addEventListener('error', () => {
    img.classList.remove('loaded');
    img.parentElement?.classList.add('bare');
  });
}
