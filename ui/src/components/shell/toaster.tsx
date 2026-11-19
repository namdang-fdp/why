import { AlertCircle, Check, X } from "lucide-react";
import { useStore } from "zustand";
import { toastStore } from "@/stores/toasts";

/** Bottom-right stack. One polite live region; each toast can be dismissed. */
export function Toaster() {
  const toasts = useStore(toastStore, (state) => state.toasts);
  const dismiss = useStore(toastStore, (state) => state.dismiss);
  return (
    <div className="toaster" role="region" aria-label="Notifications" aria-live="polite">
      {toasts.map((toast) => (
        <div key={toast.id} className="toast" data-tone={toast.tone}>
          <span className="toast-icon" aria-hidden>
            {toast.tone === "error" ? <AlertCircle size={14} /> : <Check size={14} />}
          </span>
          <div className="toast-text">
            <p className="toast-title">{toast.title}</p>
            {toast.detail && <p className="toast-detail">{toast.detail}</p>}
          </div>
          <button
            type="button"
            className="toast-close"
            aria-label="Dismiss notification"
            onClick={() => dismiss(toast.id)}
          >
            <X size={13} aria-hidden />
          </button>
        </div>
      ))}
    </div>
  );
}
