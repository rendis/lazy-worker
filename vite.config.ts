import { defineConfig } from "vite";

export default defineConfig({
  clearScreen: false,
  server: { host: "127.0.0.1", port: 1420, strictPort: true },
  // The app's content policy allows no `data:` images, so every asset stays a file.
  build: { target: ["safari15", "edge109"], assetsInlineLimit: 0 },
});
