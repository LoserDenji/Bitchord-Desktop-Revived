/// <reference types="vite/client" />

declare module '@tauri-apps/api/core' {
  export function invoke<T = unknown>(command: string, args?: Record<string, unknown>): Promise<T>
}

declare module '@tauri-apps/api/event' {
  export function listen<T = unknown>(event: string): Promise<() => Promise<void>>
  export function emit<T = unknown>(event: string, payload?: T): Promise<void>
}

interface ImportMetaEnv {
  readonly VITE_TAURI_DOMAIN: string | undefined
}

interface ImportMeta {
  readonly env: ImportMetaEnv
}