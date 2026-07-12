import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

// Tauri expects a fixed dev port and quiet output. Docs: tauri.app/start/frontend.
const host = process.env.TAURI_DEV_HOST;

export default defineConfig({
  plugins: [svelte()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    watch: {
      // src-tauri is watched by the Rust side; don't double-trigger Vite.
      ignored: ["**/src-tauri/**"],
    },
  },
});
