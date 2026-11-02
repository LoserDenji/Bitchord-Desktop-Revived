<template>
  <header class="bc-topbar">
    <div class="bc-drag" @dblclick="maximize">
      <span class="bc-logo">BitChord</span>
      <span class="bc-search">
        <input v-model="search" placeholder="Search" />
      </span>
    </div>
    <div class="bc-window-controls">
      <button class="wctrl" title="Minimize" @click="minimize">─</button>
      <button class="wctrl" title="Maximize" @click="maximize">▢</button>
      <button class="wctrl close" title="Close" @click="close">×</button>
    </div>
  </header>
</template>

<script setup lang="ts">
import { ref, watch } from 'vue'
import { invoke } from '@tauri-apps/api/core'
const props = defineProps<{ platform: string; search: string }>()
const emit = defineEmits(['update:search'])
const search = ref(props.search)
watch(search, v => emit('update:search', v))
async function minimize() { try { await invoke('minimize_window') } catch {} }
async function maximize() { try { await invoke('toggle_maximize') } catch {} }
async function close() { try { await invoke('close_window') } catch {} }
</script>

<style>
.bc-topbar {
  height: 40px; flex-shrink: 0; display: flex; align-items: center;
  justify-content: space-between; padding: 0 16px;
  background: transparent; color: var(--bc-webprimary);
  -webkit-app-region: drag;
}
.bc-drag { display: flex; align-items: center; gap: 16px; flex: 1; -webkit-app-region: drag; }
.bc-logo { font-weight: 600; font-size: 13px; }
.bc-search input {
  background: rgba(0,0,0,0.25); border: 1px solid transparent;
  color: var(--bc-desktopappletext); border-radius: 8px;
  padding: 6px 12px; font-size: 13px; width: 280px; outline: none;
}
.bc-search input:focus { border-color: var(--bc-desktopaccent); }
.bc-window-controls { display: flex; gap: 8px; -webkit-app-region: no-drag; }
.wctrl {
  width: 32px; height: 28px; border: none; border-radius: 6px;
  background: transparent; color: var(--bc-desktopappletext); font-size: 14px; cursor: pointer;
}
.wctrl:hover { background: rgba(255,255,255,0.12); }
.wctrl.close:hover { background: #ff453a; color: #fff; }
</style>
