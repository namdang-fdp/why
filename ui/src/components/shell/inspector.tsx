import { AlignLeft, MessageSquareText, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { useWorkspace } from "@/stores/workspace-context";

export function Inspector() {
  const toggle = useWorkspace((state) => state.toggleInspector);
  return (
    <aside id="why-inspector" className="inspector" aria-labelledby="why-heading">
      <div className="pane-heading">
        <MessageSquareText size={15} aria-hidden />
        <h2 id="why-heading">Why</h2>
        <Button
          variant="ghost"
          size="icon-xs"
          className="inspector-close"
          aria-label="Hide Why inspector"
          title="Hide Why inspector"
          onClick={() => {
            document.getElementById("shell-inspector")?.focus();
            toggle();
          }}
        >
          <X aria-hidden />
        </Button>
      </div>
      <div className="inspector-empty">
        <span className="empty-icon">
          <AlignLeft size={17} aria-hidden />
        </span>
        <h3>Session context</h3>
        <p>The agent session and the evidence behind each change will appear here in v0.2.</p>
      </div>
    </aside>
  );
}
