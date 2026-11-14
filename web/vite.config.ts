import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// The site is served from GitHub Pages at the apex domain (public/CNAME),
// so assets live at the root.
export default defineConfig({
  plugins: [react()],
  base: "/",
  build: { outDir: "dist", emptyOutDir: true },
});
