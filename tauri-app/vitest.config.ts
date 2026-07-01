import { defineConfig } from "vitest/config";

// Front-end unit tests are pure (no DOM/canvas) — the canvas drawing is driven
// by pure helpers in `utils.ts`. Run them in the lightweight `node` environment
// so no jsdom dependency is needed, and skip the Solid Vite plugin (which would
// otherwise pull in a browser-like test environment).
export default defineConfig({
  test: {
    environment: "node",
    include: ["src/**/*.test.ts"],
  },
});
