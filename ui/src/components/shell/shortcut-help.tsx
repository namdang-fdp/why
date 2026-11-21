import { useDialogFocus } from "@/hooks/use-dialog-focus";
import { shortcutGroups } from "@/hooks/use-workspace-shortcuts";
import { Dialog, DialogContent, DialogDescription, DialogTitle } from "@/components/ui/dialog";

export function ShortcutHelp({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const focus = useDialogFocus("shell-help");
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="shortcuts-dialog shadow-none" {...focus}>
        <DialogTitle>Keyboard shortcuts</DialogTitle>
        <DialogDescription className="sr-only">
          Keyboard shortcuts for the WHY workspace.
        </DialogDescription>
        <div className="shortcut-groups">
          {shortcutGroups.map(({ title, shortcuts }) => (
            <section key={title}>
              <h3>{title}</h3>
              <dl>
                {shortcuts.map(({ label, keys }) => (
                  <div className="shortcut-row" key={label}>
                    <dt>{label}</dt>
                    <dd>
                      {keys.map((key) => (
                        <kbd key={key}>{key}</kbd>
                      ))}
                    </dd>
                  </div>
                ))}
              </dl>
            </section>
          ))}
        </div>
        <p className="text-sm text-muted-foreground">
          Search for <strong>Set up agent hooks</strong>, <strong>Show why for this turn</strong> or{" "}
          <strong>Show unexplained files</strong> in the command palette. Why? shows the
          agent&rsquo;s own words and reads its transcript only when you open it. Tab moves through
          the preview controls, and Esc closes the dialog.
        </p>
      </DialogContent>
    </Dialog>
  );
}
