import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// @tauri-apps/cli runs `npm run dev` (vite) on a fixed port; Tauri's
// devUrl in tauri.conf.json must match. HMR over the same host keeps the
// webview reload fast during screen development.
const host = process.env.TAURI_DEV_HOST;

// https://vitejs.dev/config/
export default defineConfig({
  plugins: [react()],
  // Vite options tailored for Tauri development.
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // Tell Vite to ignore watching `src-tauri`.
      ignored: ["**/src-tauri/**"],
    },
  },
});
