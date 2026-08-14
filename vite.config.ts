import { defineConfig } from "vite";

// Tauri expects a fixed dev server URL (devUrl in tauri.conf.json),
// so the port is pinned and strictPort is on.
export default defineConfig({
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
  },
  build: {
    target: "es2022",
    outDir: "dist",
    sourcemap: false,
  },
});
