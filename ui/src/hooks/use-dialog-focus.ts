import { useRef } from "react";

export function useDialogFocus(fallbackId: string) {
  const previous = useRef<HTMLElement | null>(null);
  return {
    onOpenAutoFocus() {
      previous.current =
        document.activeElement instanceof HTMLElement ? document.activeElement : null;
    },
    onCloseAutoFocus(event: Event) {
      event.preventDefault();
      const element = previous.current;
      const target =
        element?.isConnected && element !== document.body
          ? element
          : document.getElementById(fallbackId);
      target?.focus({ preventScroll: true });
    },
  };
}
