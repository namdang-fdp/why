import { useLayoutEffect, type RefObject } from "react";

/** Move `bar` behind the row marked `aria-current="true"` in `list` (or hide it). */
function place(list: HTMLElement, bar: HTMLElement) {
  const row = list.querySelector<HTMLElement>('[aria-current="true"]');
  if (!row) {
    bar.dataset.visible = "false";
    bar.removeAttribute("data-ready");
    return;
  }
  // The list may be padded or indented, so measure the row's own box, not the list's.
  const rowBox = row.getBoundingClientRect();
  const listBox = list.getBoundingClientRect();
  const wasVisible = bar.dataset.visible === "true";
  bar.style.transform = `translate(${rowBox.left - listBox.left}px, ${rowBox.top - listBox.top}px)`;
  bar.style.width = `${rowBox.width}px`;
  bar.style.height = `${rowBox.height}px`;
  bar.dataset.visible = "true";
  if (!wasVisible) {
    // Enable the transition only after the first position has been painted.
    requestAnimationFrame(() => bar.setAttribute("data-ready", ""));
  }
}

/**
 * Makes one decorative element sit behind the selected row of a list and slide
 * to the new row when the selection moves (the "shared layout" feel of Linear's
 * lists, without a motion library).
 *
 * It only sets `transform`, `width` and `height`; the CSS (`.slide-highlight`) gives it a
 * short ease-out transition. The first placement, and a reappearance after
 * there was no selection, jump straight there (`data-ready` is added one frame
 * later), so the highlight never sweeps in from the top. `list` must be
 * `position: relative`; offsets are measured against it.
 *
 * Runs after every render (rows move when data changes) and when the list is
 * resized (sections expanding, the rail being dragged).
 */
export function useSlidingHighlight(
  list: RefObject<HTMLElement | null>,
  bar: RefObject<HTMLElement | null>,
) {
  useLayoutEffect(() => {
    const container = list.current;
    const highlight = bar.current;
    if (!container || !highlight) return;
    place(container, highlight);
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(() => place(container, highlight));
    observer.observe(container);
    return () => observer.disconnect();
  });
}
