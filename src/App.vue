<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'

const greetMsg = ref('')
const name = ref('')
const events = ref<string[]>([])

async function greet() {
  greetMsg.value = await invoke('greet', { name: name.value })
}

onMounted(async () => {
  const unlisten = await listen('frontend-ready', (event) => {
    events.value.push(`frontend-ready: ${JSON.stringify(event.payload)}`)
  })
  // keep unlisten for cleanup if needed
  void unlisten
})
</script>

<template>
  <main class="container">
    <h1>Welcome to BitChord</h1>
    <p>A Tauri + Vue 3 desktop player skeleton.</p>
    <form @submit.prevent="greet">
      <input id="greet-input" v-model="name" placeholder="Enter a name" />
      <button type="submit">Greet</button>
    </form>
    <p>{{ greetMsg }}</p>
    <ul>
      <li v-for="(e, i) in events" :key="i">{{ e }}</li>
    </ul>
  </main>
</template>

<style>
html, body {
  margin: 0;
  height: 100%;
  font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
}
.container {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  height: 100%;
  gap: 1rem;
}
input {
  padding: 0.5rem;
  width: 240px;
}
button {
  padding: 0.5rem 1rem;
}
</style>