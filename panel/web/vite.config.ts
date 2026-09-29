import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// The panel answers under a secret path that is only known at run time, so the built
// files use relative addresses ("./assets/...") and work under any prefix.
export default defineConfig({
  base: "./",
  plugins: [react()],
  build: {
    outDir: "dist",
    emptyOutDir: true,
    assetsDir: "assets",
    sourcemap: false,
    // Everything the panel needs is in its own files; nothing is fetched elsewhere.
    cssCodeSplit: false,
  },
  server: {
    // `npm run dev` talks to a panel running locally (kariz-panel serve).
    proxy: { "/api": { target: "https://127.0.0.1:28443", secure: false } },
  },
});
