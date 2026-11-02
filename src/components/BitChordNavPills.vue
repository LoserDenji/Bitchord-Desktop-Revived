<template>
  <div class="bc-navpills" v-if="narrow">
    <button v-for="dest in items" :key="dest.id"
      class="pill" :class="{ active: destination === dest.id }"
      @click="$emit('select', dest.id)">
      {{ dest.label }}
    </button>
  </div>
</template>

<script setup lang="ts">
import { ref, onMounted, onUnmounted } from 'vue'
defineProps<{ destination: string }>()
defineEmits(['select'])
const narrow = ref(false)
const items = [
  { id: 'LISTEN_NOW', label: 'Listen Now' },
  { id: 'EXPLORE', label: 'Explore' },
  { id: 'LIBRARY', label: 'Library' },
  { id: 'SEARCH', label: 'Search' },
]

function check() { narrow.value = window.innerWidth < 900 }
onMounted(() => { check(); window.addEventListener('resize', check) })
onUnmounted(() => { window.removeEventListener('resize', check) })
</script>

<style>
.bc-navpills {
  position: fixed; bottom: 82px; left: 50%; transform: translateX(-50%);
  display: flex; gap: 8px; background: var(--bc-desktopbarglass);
  padding: 6px 12px; border-radius: 20px; border: 0.5px solid var(--bc-desktopbaredge);
  backdrop-filter: blur(20px); z-index: 90;
}
.pill {
  border: none; background: transparent; color: var(--bc-websecondary);
  padding: 6px 14px; border-radius: 14px; font-size: 13px; font-weight: 500; cursor: pointer;
}
.pill.active { background: rgba(255,255,255,0.15); color: #fff; }
</style>
