import type { ReactNode } from "react";
import { ChevronRight } from "lucide-react";

/**
 * The header of a rail section. The title is a button that folds the section to
 * just this header (`aria-expanded`); `context` is a muted label next to it (the
 * selected worktree, for the file list) and `children` the controls on the right.
 */
export function RailSectionHeader({
  id,
  title,
  expanded,
  onToggle,
  context,
  contextTitle,
  children,
}: {
  id: string;
  title: string;
  expanded: boolean;
  onToggle: () => void;
  context?: string | null;
  contextTitle?: string;
  children?: ReactNode;
}) {
  return (
    <div className="rail-heading" data-collapsed={!expanded || undefined}>
      <h2 id={id}>
        <button
          type="button"
          className="rail-heading-toggle"
          aria-expanded={expanded}
          onClick={onToggle}
        >
          <ChevronRight size={12} className="disclosure" aria-hidden />
          {title}
        </button>
      </h2>
      {context && (
        <span className="rail-heading-context" title={contextTitle ?? context}>
          {context}
        </span>
      )}
      <div className="rail-heading-end">{children}</div>
    </div>
  );
}
