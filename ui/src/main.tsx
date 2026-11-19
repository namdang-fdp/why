import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "@/App";
import "@/index.css";

const favicon = document.createElement("link");
favicon.rel = "icon";
favicon.href =
  "data:image/svg+xml," +
  encodeURIComponent(
    '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><rect width="24" height="24" rx="5" fill="#252525"/><text x="12" y="17.5" fill="white" font-size="16" font-family="system-ui" font-weight="700" text-anchor="middle">W</text></svg>',
  );
document.head.append(favicon);

// Dev only: `VITE_WHY_BENCH=1 pnpm dev` times file switching (see dev/bench.ts).
if (import.meta.env.DEV && import.meta.env.VITE_WHY_BENCH) {
  void import("@/dev/bench").then(({ runBench }) =>
    runBench().catch((error: unknown) => {
      console.error("bench failed", error);
      void fetch("/__why_perf", { method: "POST", body: JSON.stringify({ error: String(error) }) });
    }),
  );
}

const root = document.getElementById("root");
if (!root) throw new Error("#root element missing from index.html");

createRoot(root).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
