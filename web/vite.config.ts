import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// The site is served from GitHub Pages at the apex domain (public/CNAME),
// so assets live at the root.
export default defineConfig({
  plugins: [react()],
  base: "/",
  // The roadmap section reads docs/ROADMAP.md (src/roadmap.ts).
  server: { fs: { allow: [".", "../docs/ROADMAP.md"] } },
  build: { outDir: "dist", emptyOutDir: true },
});
