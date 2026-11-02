<script setup lang="ts">
import { onMounted, ref, computed } from 'vue'
import { invoke } from '@tauri-apps/api/core'
// import { listen } from '@tauri-apps/api/event'
import BitChordSidebar from './components/BitChordSidebar.vue'
import BitChordPlayerBar from './components/BitChordPlayerBar.vue'
import BitChordNavPills from './components/BitChordNavPills.vue'
import BitChordTopBar from './components/BitChordTopBar.vue'

const currentDestination = ref('LISTEN_NOW')
const searchQuery = ref('')
const signedIn = ref(false)
const currentSong = ref<{ id?: string; title: string; artist: string; thumb?: string } | null>(null)
const isPlaying = ref(false)
const volume = ref(1.0)
const shuffle = ref(false)
const repeat = ref<'OFF' | 'ONE' | 'ALL'>('OFF')
const liked = ref(false)
const positionMs = ref(0)
const durationMs = ref(0)
const platform = ref('windows')

onMounted(async () => {
  try { platform.value = await invoke<string>('get_platform') } catch {}
  try {
    const state = await invoke<any>('get_playback_state')
    if (state) {
      currentSong.value = state.current_track ? { id: state.current_track, title: 'Track', artist: 'Artist' } : null
      isPlaying.value = state.is_playing ?? false
      volume.value = state.volume ?? 1.0
      if (state.repeat_mode) {
        // map RepeatMode to string
        const map: Record<string, 'OFF' | 'ALL' | 'ONE'> = { Off: 'OFF', All: 'ALL', One: 'ONE' }
        const v = map[state.repeat_mode as string] || 'OFF'
        repeat.value = v
      }
    }
  } catch {}
  // listener removed - state fetched via invoke
})

const progress = computed(() => (durationMs.value > 0 ? positionMs.value / durationMs.value : 0))

async function playPause() {
  if (isPlaying.value) { await invoke('pause_playback') }
  else { await invoke('play_track', { trackId: '00000000-0000-0000-0000-000000000000' }) }
}
async function next() { await invoke('next_track') }
async function prev() { await invoke('previous_track') }
async function seek(ms: number) { await invoke('seek_to', { positionMs: ms }) }
async function setVol(v: number) { volume.value = v; await invoke('set_volume', { volume: v }) }
async function toggleShuffle() { shuffle.value = !shuffle.value; await invoke('toggle_shuffle') }
async function toggleRepeat() {
    repeat.value = repeat.value === 'OFF' ? 'ALL' : repeat.value === 'ALL' ? 'ONE' : 'OFF'
    await invoke('toggle_repeat')
  }
function selectDest(id: string) { currentDestination.value = id }
</script>

<template>
  <div class="bc-app">
    <BitChordTopBar :platform="platform" v-model:search="searchQuery" />
    <div class="bc-body">
      <BitChordSidebar :destination="currentDestination" :signed-in="signedIn" @select="selectDest" />
      <main class="bc-content">
        <div class="bc-page">
          <h1>{{ currentDestination.replace('_', ' ') }}</h1>
          <p>BitChord Tauri shell — pixel-matched to the Compose desktop layout.</p>
        </div>
      </main>
    </div>
    <BitChordPlayerBar
      :song="currentSong" :is-playing="isPlaying" :volume="volume" :shuffle="shuffle"
      :repeat="repeat" :liked="liked" :progress="progress" :position-ms="positionMs" :duration-ms="durationMs"
      @play-pause="playPause" @next="next" @prev="prev" @seek="seek" @volume="setVol"
      @toggle-shuffle="toggleShuffle" @toggle-repeat="toggleRepeat"
    />
    <BitChordNavPills :destination="currentDestination" @select="selectDest" />
  </div>
</template>

<style>
* { box-sizing: border-box; }
html, body {
  margin: 0; height: 100%;
  background: var(--bc-desktopbackground); color: var(--bc-desktopappletext);
  font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, Helvetica, Arial, sans-serif;
  -webkit-font-smoothing: antialiased; overflow: hidden; user-select: none;
}
.bc-app { display: flex; flex-direction: column; height: 100vh; width: 100vw; }
.bc-body { display: flex; flex: 1; min-height: 0; }
.bc-content { flex: 1; min-width: 0; overflow: auto; background: var(--bc-desktopbackground); }
.bc-page { padding: 40px 56px 120px; }
.bc-page h1 { font-size: 40px; font-weight: 700; letter-spacing: -0.02em; margin: 0 0 8px; text-transform: capitalize; }
.bc-page p { color: var(--bc-websecondary); font-size: 15px; margin: 0; }
</style>
