// Runs in <head>, before first paint. Classic script on purpose: no module, no dependencies.
// 1. Apply a saved theme choice, if storage is readable (it can throw in private windows).
// 2. Turn on the scroll-reveal start state only when motion is allowed and the browser can
//    observe intersections. If the reveal script has not started after 2.5 s (blocked, failed,
//    slow), turn it off again so no content is ever stuck hidden.
(function () {
  var root = document.documentElement;
  try {
    var saved = localStorage.getItem("why-theme");
    if (saved === "dark" || saved === "light") root.setAttribute("data-theme", saved);
  } catch (e) {}
  try {
    var calm = window.matchMedia("(prefers-reduced-motion: no-preference)").matches;
    if (calm && "IntersectionObserver" in window) {
      root.classList.add("reveal");
      setTimeout(function () {
        if (!window.__revealReady) root.classList.remove("reveal");
      }, 2500);
    }
  } catch (e) {}
})();
