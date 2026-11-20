import { useEffect, useState, type ReactNode } from "react";

/** Keep in step with `--dur-slow` in index.css. */
const CLOSE_MS = 200;

/**
 * Height-animated disclosure. The body is a grid row that goes between `0fr`
 * and `1fr` (`.collapsible` in index.css), which animates to the content's real
 * height with no measuring. After the closing animation the children are
 * unmounted, so hidden rows leave the tab order and the accessibility tree.
 * The first render (page load) does not animate: `data-animate` is only set once
 * `open` has actually changed.
 */
export function Collapsible({ open, children }: { open: boolean; children: ReactNode }) {
  const [wasOpen, setWasOpen] = useState(open);
  const [mounted, setMounted] = useState(open);
  const [animate, setAnimate] = useState(false);

  // Derived state, adjusted during render (React's documented pattern for "a prop changed").
  if (open !== wasOpen) {
    setWasOpen(open);
    setAnimate(true);
    if (open) setMounted(true);
  }
  // A close timer's setState can already be queued when an open lands (a click within a few ms
  // of it firing, or a slow main thread) and then apply AFTER the `setMounted(true)` above;
  // clearTimeout cannot recall an update that is already queued. So repair `mounted` whenever
  // it disagrees with `open`, not only on the render where `open` changed.
  if (open && !mounted) setMounted(true);

  useEffect(() => {
    if (open) return;
    const timer = setTimeout(() => setMounted(false), CLOSE_MS);
    return () => clearTimeout(timer);
  }, [open]);

  // `open` alone must be enough to render: a stale `mounted=false` must never hide an open body.
  if (!open && !mounted) return null;
  return (
    <div
      className="collapsible"
      data-state={open ? "open" : "closed"}
      data-animate={animate || undefined}
    >
      <div className="collapsible-inner">{children}</div>
    </div>
  );
}
