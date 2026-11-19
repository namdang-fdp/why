import { AlertCircle } from "lucide-react";
import type { IpcError } from "@/contracts";
import { Button } from "@/components/ui/button";
import { errorMessages, errorTitles } from "@/lib/error-text";

export function ErrorMessage({
  error,
  retry,
  context,
}: {
  error: IpcError;
  retry?: () => void;
  context?: string;
}) {
  return (
    <div className="async-error" role="alert">
      <div className="error-heading">
        <AlertCircle size={14} aria-hidden />
        <strong>{errorTitles[error.kind]}</strong>
        {context && <span className="error-context">{context}</span>}
      </div>
      <p>{errorMessages[error.kind]}</p>
      {"message" in error && <p className="error-detail">{error.message}</p>}
      <code className="error-kind">{error.kind}</code>
      {retry && (
        <Button variant="ghost" size="xs" onClick={retry}>
          Try again
        </Button>
      )}
    </div>
  );
}

export function LoadingMessage({ children }: { children: string }) {
  return (
    <p className="rail-note" role="status">
      {children}
    </p>
  );
}
