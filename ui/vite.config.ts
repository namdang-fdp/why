import path from "node:path";
import { fileURLToPath } from "node:url";
import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import type { Plugin } from "vite";
import { defineConfig } from "vitest/config";
import { devPort } from "./dev-port.ts";

/**
 * Dev server only: prints the benchmark report posted by `src/dev/bench.ts`, so
 * timings measured inside the Tauri window can be read in the terminal. Not part
 * of a build (`apply: "serve"`).
 */
function perfSink(): Plugin {
  return {
    name: "why-perf-sink",
    apply: "serve",
    configureServer(server) {
      server.middlewares.use("/__why_perf", (request, response) => {
        let body = "";
        request.on("data", (chunk: Buffer) => (body += chunk.toString()));
        request.on("end", () => {
          console.log(`[why-perf] ${body}`);
          response.statusCode = 204;
          response.end();
        });
      });
    },
  };
}

const root = path.dirname(fileURLToPath(import.meta.url));

export default defineConfig({
  // Vite's project root is `ui/`, so index.html lives next to src/.
  root,
  plugins: [react(), tailwindcss(), perfSink()],
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
