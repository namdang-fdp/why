import tailwindcss from "@tailwindcss/vite";
import { defineConfig } from "astro/config";
import { release } from "./src/data/release.ts";

// Static output, no Astro integrations. Styles and scripts are never inlined, so the CSP in
// vercel.json can stay free of 'unsafe-inline'.
export default defineConfig({
  site: release.site,
  output: "static",
  build: { inlineStylesheets: "never" },
  vite: {
    plugins: [tailwindcss()],
    build: { assetsInlineLimit: 0 },
  },
});
