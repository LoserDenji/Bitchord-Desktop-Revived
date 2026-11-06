import { useMutation, useQuery } from '@tanstack/react-query';
import type { DetectedExtension } from '@bitchord/shared/bridge';
import { queryClient } from '@/lib/queryClient';
import { extensionQueryKey, useExtension } from './useExtension';

/**
 * The Dynamic Island — the desktop shell's floating now-playing card — as a
 * settings row. The shell reports it under the `island` capability, which a
 * browser extension never does, so the toggle only renders in the desktop app;
 * everywhere else this reads "unavailable" and the section stays hidden.
 */

export const islandQueryKey = ['island', 'state'] as const;

export interface IslandState {
  enabled: boolean;
}

function detected(): DetectedExtension | null {
  return queryClient.getQueryData<DetectedExtension | null>(extensionQueryKey) ?? null;
}

const supported = (extension: DetectedExtension | null) => !!extension?.info.capabilities.includes('island');

const adopt = (state: IslandState) => queryClient.setQueryData(islandQueryKey, state);

async function readState(): Promise<IslandState> {
  const extension = detected();
  if (!extension || !supported(extension)) return { enabled: false };
  const state = await extension.bridge.call('island.state', {});
  adopt(state);
  return state;
}

export type IslandStatus = { state: 'checking' } | { state: 'unavailable' } | { state: 'ready'; island: IslandState };

export function useIsland(): IslandStatus {
  const extension = useExtension();
  const present = extension.state === 'present' ? extension.extension : null;
  const available = supported(present);
  const query = useQuery({ queryKey: islandQueryKey, queryFn: readState, enabled: available, staleTime: 30_000 });

  if (extension.state === 'checking' || (available && query.isPending)) return { state: 'checking' };
  if (!available) return { state: 'unavailable' };
  return { state: 'ready', island: query.data ?? { enabled: true } };
}

/** The toggle. Optimistic: the overlay window appears (or vanishes) as fast
 * as the click, and the response — which the shell answers after it has built
 * or torn the window down — settles the row to what the app actually did. */
export function useIslandEnabled() {
  return useMutation({
    mutationFn: (on: boolean) => {
      const extension = detected();
      if (!extension || !supported(extension)) return Promise.reject(new Error('The Dynamic Island is not available here'));
      return extension.bridge.call('island.setEnabled', { enabled: on });
    },
    onMutate: (on: boolean) => adopt({ enabled: on }),
    onSuccess: adopt,
  });
}
