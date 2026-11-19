import { AlertCircle } from "lucide-react";
import type { IpcError } from "@/contracts";
import { Button } from "@/components/ui/button";

const errorMessages = {
  notARepo: "This location is no longer a Git repository.",
  gitMissing: "Git is unavailable. Check that Git is installed and on your PATH.",
  gitFailed: "Git could not complete the request.",
  timeout: "The request took too long.",
  notFound: "The requested repository, worktree, or file could not be found.",
  invalidInput: "This request is unavailable for the current selection.",
  internal: "WHY could not complete the request because of an internal error.",
} satisfies Record<IpcError["kind"], string>;

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
        {context && <span>{context}</span>}
        <code>{error.kind}</code>
      </div>
      <p>{errorMessages[error.kind]}</p>
      {"message" in error && <p className="error-detail">{error.message}</p>}
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
