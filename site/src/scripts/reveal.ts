// Scroll reveal with AnimeJS: progressive enhancement. The hidden start state only exists while
// `html.reveal` is set (public/theme-init.js sets it when motion is allowed). If anything here
// fails, the class is removed so no content stays hidden.
import { animate } from "animejs/animation";

declare global {
  interface Window {
    __revealReady?: boolean;
  }
}

const root = document.documentElement;
// Tell theme-init.js that this script started, so it does not switch the reveal state off.
window.__revealReady = true;

function show(el: HTMLElement, delay: number) {
  animate(el, {
    opacity: [0, 1],
    translateY: [20, 0],
    filter: ["blur(4px)", "blur(0px)"],
    duration: 600,
    delay,
    ease: "outCubic",
    onComplete: () => {
      // Hand control back to the stylesheet: the element is now in its normal state.
      el.classList.add("is-revealed");
      el.style.removeProperty("opacity");
      el.style.removeProperty("transform");
      el.style.removeProperty("filter");
    },
  });
}

try {
  if (root.classList.contains("reveal")) {
    const observer = new IntersectionObserver(
      (entries) => {
        entries
          .filter((entry) => entry.isIntersecting)
          .forEach((entry, index) => {
            observer.unobserve(entry.target);
            // Items that enter together arrive in a short stagger, capped so it never drags.
            show(entry.target as HTMLElement, Math.min(index, 4) * 70);
          });
      },
      { rootMargin: "0px 0px -8% 0px", threshold: 0.1 },
    );
    document.querySelectorAll<HTMLElement>("[data-reveal]").forEach((el) => observer.observe(el));
  }
} catch {
  root.classList.remove("reveal");
}
