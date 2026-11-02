<template>
  <div class="bc-player">
    <div class="bc-player__pill">
      <div class="bc-player__transport">
        <button class="tbtn" :class="{ active: shuffle }" title="Shuffle" @click="$emit('toggle-shuffle')">🔀</button>
        <button class="tbtn" title="Previous" @click="$emit('prev')">⏮</button>
        <button class="tbtn play" title="Play/Pause" @click="$emit('play-pause')">{{ isPlaying ? '⏸' : '▶' }}</button>
        <button class="tbtn" title="Next" @click="$emit('next')">⏭</button>
        <button class="tbtn" :class="{ active: repeat !== 'OFF' }" title="Repeat" @click="$emit('toggle-repeat')">🔁</button>
      </div>

      <div class="bc-player__lcd" @click="$emit('open-now-playing')">
        <div class="bc-player__cover">
          <img v-if="song?.thumb" :src="song.thumb" alt="cover" />
          <div v-else class="bc-player__placeholder">🎵</div>
        </div>
        <div class="bc-player__meta">
          <span class="bc-player__title">{{ song?.title || 'BitChord' }}</span>
          <span class="bc-player__artist">{{ song?.artist || 'Not Playing' }}</span>
        </div>
        <div class="bc-player__progress">
          <div class="bc-player__bar" :style="{ width: (progress * 100) + '%' }"></div>
        </div>
      </div>

      <div class="bc-player__actions">
        <button class="abtn" :class="{ active: liked }" title="Favourite" @click="$emit('toggle-like')">★</button>
        <button class="abtn" title="Lyrics" @click="$emit('open-lyrics')">🎤</button>
        <button class="abtn" title="Queue" @click="$emit('open-queue')">≡</button>
        <div class="bc-player__vol">
          <input type="range" min="0" max="1" step="0.01" :value="volume" @input="(e: Event) => $emit('volume', Number((e.target as HTMLInputElement).value))" />
        </div>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
defineProps<{
  song?: { id?: string; title: string; artist: string; thumb?: string } | null
  isPlaying: boolean
  volume: number
  shuffle: boolean
  repeat: string
  liked: boolean
  progress: number
  positionMs: number
  durationMs: number
}>()
defineEmits(['play-pause', 'next', 'prev', 'seek', 'volume', 'toggle-shuffle', 'toggle-repeat', 'toggle-like', 'open-now-playing', 'open-lyrics', 'open-queue'])
</script>

<style>
.bc-player {
  position: fixed; bottom: 20px; left: 50%; transform: translateX(-50%);
  width: calc(100% - 32px); max-width: 668px; height: 56px; z-index: 100;
}
.bc-player__pill {
  width: 100%; height: 100%; border-radius: 28px;
  background: var(--bc-desktopbarglass);
  border: 0.5px solid var(--bc-desktopbaredge);
  box-shadow: 0 20px 40px rgba(0,0,0,0.3);
  backdrop-filter: blur(20px);
  display: flex; align-items: center; padding: 0 16px; gap: 12px;
}
.bc-player__transport { display: flex; align-items: center; gap: 6px; width: 184px; }
.tbtn { width: 28px; height: 28px; border: none; border-radius: 6px; background: transparent; color: var(--bc-webprimary); cursor: pointer; font-size: 14px; }
.tbtn.active { color: var(--bc-desktopaccent); }
.tbtn.play { font-size: 16px; }
.bc-player__lcd {
  flex: 1; height: 40px; border-radius: 6px; background: rgba(0,0,0,0.2);
  display: flex; align-items: center; gap: 8px; padding: 0 8px; position: relative; cursor: pointer;
}
.bc-player__cover { width: 34px; height: 34px; border-radius: 6px; overflow: hidden; background: #2a2a2e; }
.bc-player__cover img { width: 100%; height: 100%; object-fit: cover; }
.bc-player__placeholder { display: flex; align-items: center; justify-content: center; height: 100%; font-size: 18px; }
.bc-player__meta { display: flex; flex-direction: column; flex: 1; min-width: 0; }
.bc-player__title { font-size: 13px; font-weight: 600; color: var(--bc-webprimary); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.bc-player__artist { font-size: 12px; color: var(--bc-websecondary); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.bc-player__progress { position: absolute; bottom: 0; left: 0; right: 0; height: 2px; background: rgba(255,255,255,0.1); border-radius: 1px; }
.bc-player__bar { height: 100%; background: #fff; border-radius: 1px; transition: width 0.2s linear; }
.bc-player__actions { display: flex; align-items: center; gap: 8px; width: 132px; justify-content: flex-end; }
.abtn { width: 24px; height: 28px; border: none; background: transparent; color: var(--bc-webprimary); cursor: pointer; font-size: 14px; }
.abtn.active { color: var(--bc-desktopaccent); }
.bc-player__vol input { width: 60px; accent-color: var(--bc-desktopaccent); }
</style>
