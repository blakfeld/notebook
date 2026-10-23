import { svelte } from '@sveltejs/vite-plugin-svelte'
import { defineConfig } from 'vite'

// Set by the Tauri CLI during `ios dev` / `android dev` so a physical device
// can reach the dev server over the network.
const host = process.env.TAURI_DEV_HOST

// Tauri's devUrl expects the dev server on a fixed port.
export default defineConfig({
  plugins: [svelte()],
  // Component tests mount Svelte's client build, not its server one.
  resolve: process.env.VITEST ? { conditions: ['browser'] } : undefined,
  clearScreen: false,
  // Fonts stay files: the CSP's `default-src 'self'` refuses `data:` fonts.
  build: {
    assetsInlineLimit: (file) => (file.endsWith('.woff2') ? false : undefined),
  },
  server: {
    port: 5173,
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: 'ws', host, port: 5174 } : undefined,
  },
})
