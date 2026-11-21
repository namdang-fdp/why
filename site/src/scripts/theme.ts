// Theme toggle: progressive enhancement. Without JavaScript the page follows the OS through
// `prefers-color-scheme` (see global.css) and this button stays hidden.
const KEY = "why-theme";
const root = document.documentElement;
const button = document.getElementById("theme-toggle");

function current(): "dark" | "light" {
  const set = root.getAttribute("data-theme");
  if (set === "dark" || set === "light") return set;
  return matchMedia("(prefers-color-scheme: light)").matches ? "light" : "dark";
}

if (button) {
  button.hidden = false;
  button.addEventListener("click", () => {
    const next = current() === "dark" ? "light" : "dark";
    root.setAttribute("data-theme", next);
    // Storage can throw (private windows, blocked site data). The choice then lasts for this
    // visit only, and the page keeps working.
    try {
      localStorage.setItem(KEY, next);
    } catch {
      /* ignore */
    }
  });
}
