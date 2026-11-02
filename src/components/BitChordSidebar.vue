<template>
  <aside class="bc-sidebar" :class="{ rail: rail }">
    <div class="bc-sidebar__top">
      <div class="bc-sidebar__caption">
        <span class="bc-drag">BitChord</span>
        <button class="bc-icon-btn" title="Collapse" @click="rail = !rail">‹</button>
      </div>
    </div>

    <nav class="bc-sidebar__nav">
      <button v-for="item in items" :key="item.id" class="bc-sidebar__item"
        :class="{ active: destination === item.id }" @click="$emit('select', item.id)">
        <span class="bc-sidebar__glyph" v-html="item.icon"></span>
        <span class="bc-sidebar__label">{{ item.label }}</span>
      </button>
    </nav>

    <div class="bc-sidebar__footer">
      <button class="bc-sidebar__item" @click="$emit('select', 'SETTINGS')">
        <span class="bc-sidebar__glyph">⚙</span><span class="bc-sidebar__label">Settings</span>
      </button>
      <button class="bc-account" @click="$emit('select', 'ACCOUNT')">
        <span class="bc-avatar">●</span><span class="bc-account__label">Account</span>
      </button>
    </div>
  </aside>
</template>

<script setup lang="ts">
import { ref } from 'vue'
defineProps<{ destination: string; signedIn?: boolean }>()
defineEmits(['select'])
const rail = ref(false)
const items = [
  { id: 'SEARCH', label: 'Search', icon: '🔍' },
  { id: 'LISTEN_NOW', label: 'Listen Now', icon: '♩' },
  { id: 'EXPLORE', label: 'Explore', icon: '✦' },
  { id: 'HISTORY', label: 'History', icon: '🕒' },
  { id: 'DOWNLOADS', label: 'Downloads', icon: '⬇' },
  { id: 'LOCAL_MUSIC', label: 'Local Music', icon: '♪' },
]
</script>

<style>
.bc-sidebar {
  width: 260px; flex-shrink: 0; height: 100%;
  background: var(--bc-desktopglass);
  border-right: 0.5px solid rgba(255,255,255,0.05);
  display: flex; flex-direction: column;
  font-size: 14px;
  -webkit-app-region: drag;
}
.bc-sidebar.rail { width: 72px; }
.bc-sidebar__caption {
  height: 40px; display: flex; align-items: center;
  justify-content: space-between; padding: 0 10px;
  -webkit-app-region: drag;
}
.bc-drag { color: var(--bc-webprimary); font-weight: 600; font-size: 13px; cursor: grab; }
.bc-icon-btn { background: none; border: none; color: var(--bc-websecondary); font-size: 18px; cursor: pointer; }
.bc-sidebar__nav { flex: 1; padding: 4px 8px; overflow-y: auto; }
.bc-sidebar__item {
  display: flex; align-items: center; gap: 8px; width: 100%;
  height: 36px; padding: 0 4px; border: none; border-radius: 8px;
  background: transparent; color: var(--bc-appletext); cursor: pointer;
  font-size: 14px; text-align: left;
}
.bc-sidebar__item { -webkit-app-region: no-drag; }
  .bc-sidebar__item:hover { background: rgba(255,255,255,0.05); }
.bc-sidebar__item.active { background: rgba(255,255,255,0.05); }
.bc-sidebar__glyph { width: 24px; text-align: center; font-size: 18px; }
.bc-sidebar__label { flex: 1; }
.bc-sidebar.rail .bc-sidebar__label { display: none; }
.bc-sidebar__footer { padding: 8px; border-top: 0.5px solid rgba(255,255,255,0.08); }
.bc-account { display: flex; align-items: center; gap: 8px; width: 100%;
  height: 36px; padding: 0 4px; border: none; border-radius: 8px; background: transparent; cursor: pointer; -webkit-app-region: no-drag; }
.bc-avatar { width: 28px; height: 28px; border-radius: 50%; background: var(--bc-desktopaccent); color: #fff; display: flex; align-items: center; justify-content: center; }
</style>
