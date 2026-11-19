import { createStore } from "zustand/vanilla";

export type Toast = {
  id: number;
  tone: "info" | "error";
  title: string;
  detail?: string;
};

type ToastInput = Omit<Toast, "id">;

/** How long a toast stays: errors need more time to be read and acted on. */
const lifetimeMs = { info: 4000, error: 8000 } as const;

export type Toasts = {
  toasts: readonly Toast[];
  push: (toast: ToastInput) => number;
  dismiss: (id: number) => void;
};

export function createToastStore() {
  let nextId = 1;
  return createStore<Toasts>((set, get) => ({
    toasts: [],
    push: (toast) => {
      const id = nextId++;
      // Newest last; the viewport stacks them bottom-up. Cap the pile so a
      // storm of errors cannot cover the window.
      set({ toasts: [...get().toasts, { ...toast, id }].slice(-4) });
      setTimeout(() => get().dismiss(id), lifetimeMs[toast.tone]);
      return id;
    },
    dismiss: (id) => set({ toasts: get().toasts.filter((toast) => toast.id !== id) }),
  }));
}

/** The app-wide store behind the `<Toaster />`. Tests build their own. */
export const toastStore = createToastStore();
