import { useEffect, useState } from 'react';

/**
 * Whether this window ships the app's own transparent top bar: a frameless
 * shell with the page drawing the drag zone and the window buttons. True only
 * on the desktop shell's Windows and Linux builds — the native frame is
 * removed there (lib.rs) and something must own the minimize/maximize/close
 * gestures. macOS keeps its traffic lights over the glass (TitleBarStyle::
 * Overlay), and plain browsers were never frameless. The answer arrives with
 * the shell's `desktop_info` round trip, so a fresh window doesn't guess
 * wrong for a frame; every reader re-renders when it lands.
 */

type Internals = {
  invoke: (cmd: string, args?: Record<string, unknown>) => Promise<unknown>;
};

type DesktopInfo = { platform: string };

let cached: boolean | null = null;
const waiters = new Set<(present: boolean) => void>();

function publish(value: boolean) {
  cached = value;
  for (const waiter of waiters) waiter(value);
}

/** Asks the shell once; in a browser there is no shell and never a bar. */
function probe() {
  const internals = (window as unknown as { __TAURI_INTERNALS__?: Internals }).__TAURI_INTERNALS__;
  if (!internals) {
    publish(false);
    return;
  }
  void internals
    .invoke('desktop_info')
    .then((info) => publish((info as DesktopInfo).platform !== 'macos'))
    .catch(() => publish(false));
}

export function useWindowChrome(): boolean {
  const [present, setPresent] = useState(cached ?? false);
  useEffect(() => {
    if (cached !== null) {
      setPresent(cached);
      return;
    }
    waiters.add(setPresent);
    if (cached === null && waiters.size === 1) probe();
    return () => {
      waiters.delete(setPresent);
    };
  }, []);
  return present;
}
