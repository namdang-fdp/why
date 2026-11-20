import { expect, it } from "vitest";
// The fragment bypasses Vitest’s empty CSS module stub so this reads the source.
import styles from "@/styles/sessions.css#?raw";

it("disables the live pulse under reduced motion and keeps timeline scroll instant", () => {
  expect(styles).toMatch(
    /\.session-status\[data-status="live"\] > span\s*\{[^}]*animation: session-live-pulse/,
  );
  expect(styles).toMatch(
    /@media \(prefers-reduced-motion: reduce\)\s*\{\s*\.session-status\[data-status="live"\] > span\s*\{\s*animation: none !important;/,
  );
  expect(styles).toMatch(/\.session-scroll\s*\{[^}]*scroll-behavior: auto;/);
});
