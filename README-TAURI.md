# BitChord Tauri + Vue 3 Desktop Player

A Tauri + Vue 3 skeleton for building desktop applications with Rust backend.

## Prerequisites

- **Node.js** 18+ (tested with v24.14.0)
- **npm** (or pnpm/yarn)
- **Rust** + **Cargo** (for Tauri backend)
- **Tauri CLI**: `cargo install tauri-cli`

## Project Structure

```
├── src/                    # Vue 3 frontend
│   ├── main.ts            # App entry point
│   ├── App.vue            # Root component
│   └── vite-env.d.ts      # TypeScript declarations
│
├── public/                # Static assets (served as-is)
│   └── favicon.svg
│
├── src-tauri/             # Rust Tauri backend
│   ├── Cargo.toml
│   ├── build.rs
│   ├── tauri.conf.json
│   └── src/
│       ├── main.rs        # Tauri entry point and commands
│       └── lib.rs         # Additional Rust commands module
│
├── icons/                 # App icons for bundling
│
├── package.json
├── tsconfig.json
├── vite.config.ts
└── tauri.conf.json        # Root-level Tauri config (alternative)
```

## Getting Started

### Install dependencies

```bash
npm install
```

### Development

Run the Vue dev server:
```bash
npm run dev
```

Run the Tauri desktop app (requires Rust):
```bash
npm run tauri:dev
```

### Build

Build the frontend:
```bash
npm run build
```

Build the desktop app (produces distributable for current platform):
```bash
npm run tauri:build
```

## Available Scripts

| Script | Description |
|--------|-------------|
| `npm run dev` | Start Vite dev server (http://localhost:1420) |
| `npm run build` | Build frontend for production |
| `npm run tauri:dev` | Run Tauri desktop app in dev mode |
| `npm run tauri:build` | Build Tauri app for current platform |

## Tauri Commands

The project includes example Tauri commands accessible from Vue:

- `greet(name)` - Returns a greeting message
- `get_platform()` - Returns the current platform (windows/macOS/linux)

### Using Tauri API from Vue

```ts
import { invoke } from '@tauri-apps/api/core'

// Call a Rust command
const greeting = await invoke('greet', { name: 'World' })

// Open external URL
import { open } from '@tauri-apps/api/plugin'
await open('https://example.com')
```

## Build Targets

See `tauri.conf.json` → `bundle` → `targets` for available platforms.

## License

MIT