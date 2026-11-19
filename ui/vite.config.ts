import path from "node:path";
import { fileURLToPath } from "node:url";
import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";
import { devPort } from "./dev-port.ts";

const root = path.dirname(fileURLToPath(import.meta.url));

export default defineConfig({
  // Vite's project root is `ui/`, so index.html lives next to src/.
  root,
  plugins: [react(), tailwindcss()],
  resolve: { alias: { "@": path.resolve(root, "src") } },
  // Tauri needs a fixed port (it points the webview at it); fail instead of picking another.
  server: {
    port: devPort(),
    strictPort: true,
    host: "localhost",
    // Don't reload the UI when Rust files change; Tauri restarts the app itself.
    watch: { ignored: ["**/src-tauri/**"] },
  },
  build: { outDir: "dist", emptyOutDir: true },
  test: { environment: "node", include: ["src/**/*.test.ts"] },
});
