import { useEffect, useRef, useState, type FormEvent } from "react";
import { FolderOpen, Loader2, TriangleAlert } from "lucide-react";
import type { IpcError } from "@/contracts";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogTitle } from "@/components/ui/dialog";
import { useDialogFocus } from "@/hooks/use-dialog-focus";
import { describeAddError } from "@/lib/error-text";
import { useWorkspace } from "@/stores/workspace-context";

function AddRepositoryForm({ close }: { close: () => void }) {
  const [path, setPath] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<IpcError | null>(null);
  const storage = useWorkspace((state) => state.storage);
  const pick = useWorkspace((state) => state.pickRepositoryFolder);
  const add = useWorkspace((state) => state.addRepository);

  // This form lives only while its dialog is open. A request it started can
  // finish after the dialog was dismissed (and even re-opened as a NEW form), and
  // must then neither close that new dialog nor write into a form that is gone.
  const alive = useRef(true);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  async function submit(target: string) {
    // Trim only to refuse an empty input. The path itself goes through exactly as
    // given: a directory name may legitimately start or end with a space.
    if (!target.trim() || busy) return;
    setBusy(true);
    setError(null);
    const result = await add(target);
    if (!alive.current) return; // the repository was still added; the toast says so
    setBusy(false);
    if (result.status === "ok") close();
    else setError(result.error);
  }

  async function browse() {
    if (busy) return;
    setBusy(true);
    setError(null);
    const picked = await pick();
    if (!alive.current) return;
    setBusy(false);
    if (picked.status === "error") setError(picked.error);
    // Choosing a folder is decisive: add it straight away. Cancelling does nothing.
    else if (picked.data !== null) {
      setPath(picked.data);
      await submit(picked.data);
    }
  }

  function onSubmit(event: FormEvent) {
    event.preventDefault();
    void submit(path);
  }

  const temporary = storage.status === "ok" && !storage.data.persistent;

  return (
    <form className="add-form" onSubmit={onSubmit}>
      <DialogTitle>Add repository</DialogTitle>
      <DialogDescription>
        Choose the folder of a Git repository. WHY reads it and all its worktrees; it never changes
        them.
      </DialogDescription>

      {temporary && (
        <p className="notice" role="note">
          <TriangleAlert size={14} aria-hidden />
          <span>
            Repositories won&apos;t be saved this session, so you will need to add this one again
            next time.
          </span>
        </p>
      )}

      <div className="field">
        <label htmlFor="add-repository-path">Folder path</label>
        <div className="field-row">
          <input
            id="add-repository-path"
            className="input mono"
            value={path}
            onChange={(event) => setPath(event.target.value)}
            placeholder="/path/to/repository"
            spellCheck={false}
            autoComplete="off"
            aria-invalid={error ? true : undefined}
            aria-describedby={error ? "add-repository-error" : undefined}
          />
          <Button type="button" variant="outline" size="sm" onClick={() => void browse()}>
            <FolderOpen aria-hidden />
            Browse…
          </Button>
        </div>
        {error && (
          <p id="add-repository-error" className="field-error" role="alert">
            {describeAddError(error)}
          </p>
        )}
      </div>

      <div className="modal-actions">
        <Button type="button" variant="ghost" size="sm" onClick={close}>
          Cancel
        </Button>
        <Button type="submit" size="sm" disabled={!path.trim() || busy}>
          {busy && <Loader2 className="spin" aria-hidden />}
          Add repository
        </Button>
      </div>
    </form>
  );
}

export function AddRepositoryDialog() {
  const open = useWorkspace((state) => state.addRepositoryOpen);
  const setOpen = useWorkspace((state) => state.setAddRepositoryOpen);
  const focus = useDialogFocus("add-repository-trigger");
  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogContent className="add-dialog shadow-none" showCloseButton={false} {...focus}>
        <AddRepositoryForm close={() => setOpen(false)} />
      </DialogContent>
    </Dialog>
  );
}
