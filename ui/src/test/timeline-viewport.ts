/** jsdom has no layout. Give the timeline a viewport that clamps scrollTop like a browser. */
export function mockTimelineViewport(viewport: HTMLElement, height: number) {
  let scrollTop = 0;
  const content = viewport.querySelector<HTMLElement>(".session-window");
  const scrollHeight = () => Math.max(height, Number.parseFloat(content?.style.height || "0"));
  Object.defineProperties(viewport, {
    clientHeight: { configurable: true, value: height },
    scrollHeight: { configurable: true, get: scrollHeight },
    scrollTop: {
      configurable: true,
      get: () => Math.min(scrollTop, scrollHeight() - height),
      set: (top: number) => {
        scrollTop = Math.max(0, Math.min(top, scrollHeight() - height));
      },
    },
  });
}
