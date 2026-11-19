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

  useEffect(() => {
    if (open) return;
    const timer = setTimeout(() => setMounted(false), CLOSE_MS);
    return () => clearTimeout(timer);
  }, [open]);

  if (!mounted) return null;
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
