import { Fragment, useContext, useLayoutEffect, useRef, type KeyboardEvent } from "react";
import { ArrowLeft, MessageSquareText, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { SessionList, TaskHeader } from "@/components/sessions/session-details";
import { SessionTimeline } from "@/components/sessions/session-timeline";
import { moveTurn } from "@/components/sessions/session-controls";
import { WorkspaceContext, useSessions, useWorkspace } from "@/stores/workspace-context";
import { ErrorMessage, LoadingMessage } from "./async-message";
import { FileActivity } from "./file-activity";

export function Inspector() {
  const workspace = useContext(WorkspaceContext);
  const toggle = useWorkspace((state) => state.toggleInspector);
  const list = useSessions((state) => state.list);
  const sessionId = useSessions((state) => state.sessionId);
  const timeline = useSessions((state) => state.timeline);
  const selectSession = useSessions((state) => state.selectSession);
  const loadList = useSessions((state) => state.loadList);
  const loadTimeline = useSessions((state) => state.loadTimeline);
  const panel = useRef<HTMLElement>(null);
  const backButton = useRef<HTMLButtonElement>(null);
  const restore = useRef<string | null>(null);

  function open(id: string) {
    restore.current = id;
    panel.current?.focus({ preventScroll: true });
    void selectSession(id);
  }

  function back() {
    panel.current?.focus({ preventScroll: true });
    void selectSession(null);
  }

  useLayoutEffect(() => {
    if (!panel.current || document.activeElement !== panel.current) return;
    if (sessionId !== null && restore.current === sessionId) {
      backButton.current?.focus({ preventScroll: true });
    } else if (sessionId === null && restore.current !== null && list.status === "ok") {
      const id = restore.current;
      restore.current = null;
      const rows = [...panel.current.querySelectorAll<HTMLButtonElement>("[data-session-id]")];
      (rows.find((row) => row.dataset.sessionId === id) ?? rows[0])?.focus();
    }
  }, [list, sessionId]);

  function keyDown(event: KeyboardEvent<HTMLElement>) {
    if (
      event.defaultPrevented ||
      event.nativeEvent.isComposing ||
      event.altKey ||
      event.ctrlKey ||
      event.metaKey ||
      event.shiftKey ||
      !(event.target instanceof Node) ||
      !event.currentTarget.contains(event.target)
    )
      return;
    const key = event.key;
    if (key === "Escape" && sessionId !== null) {
      event.preventDefault();
      back();
      return;
    }
    if (sessionId !== null && key === "w") {
      // Show why for the selected turn (the agent's words, read only now).
      const { turnId } = workspace.getState().sessions.getState();
      if (turnId !== null && turnId !== "") {
        event.preventDefault();
        workspace.getState().evidence.getState().toggleWhy({ kind: "turn", sessionId, turnId });
      }
      return;
    }
    if (sessionId !== null && (key === "n" || key === "p")) {
      event.preventDefault();
      moveTurn(workspace, key === "n" ? 1 : -1, true);
      return;
    }
    const direction =
      key === "j" || key === "ArrowDown"
        ? 1
        : key === "k" || key === "ArrowUp"
          ? -1
          : key === "Home"
            ? "first"
            : key === "End"
              ? "last"
              : null;
    if (direction === null) return;
    // Leave the scrollport's native arrow and page scrolling intact.
    if (
      key.startsWith("Arrow") &&
      event.target instanceof Element &&
      event.target.matches(".session-scroll")
    )
      return;
    event.preventDefault();
    if (sessionId !== null) {
      moveTurn(workspace, direction);
      return;
    }
    const rows = [...event.currentTarget.querySelectorAll<HTMLButtonElement>("[data-session-id]")];
    const current = rows.findIndex((row) => row === document.activeElement);
    const index =
      direction === "first"
        ? 0
        : direction === "last"
          ? rows.length - 1
          : current === -1
            ? direction === 1
              ? 0
              : rows.length - 1
            : Math.max(0, Math.min(rows.length - 1, current + direction));
    rows[index]?.focus();
  }

  return (
    <aside
      id="why-inspector"
      className="inspector"
      aria-labelledby="why-heading"
      aria-keyshortcuts="Shift+I"
      ref={panel}
      tabIndex={-1}
      onKeyDown={keyDown}
    >
      <div className="pane-heading">
        <MessageSquareText size={15} aria-hidden />
        <h2 id="why-heading">Why</h2>
        {sessionId !== null && (
          <button
            type="button"
            className="session-text-button session-back"
            ref={backButton}
            onClick={back}
          >
            <ArrowLeft size={13} aria-hidden /> Sessions
          </button>
        )}
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
      <FileActivity />
      {sessionId === null ? (
        <>
          {list.status === "idle" && (
            <p className="rail-note">Select a worktree to see its sessions.</p>
          )}
          {list.status === "loading" && <LoadingMessage>Loading sessions…</LoadingMessage>}
          {list.status === "error" && (
            <ErrorMessage context="Sessions" error={list.error} retry={() => void loadList()} />
          )}
          {list.status === "ok" && <SessionList sessions={list.data} onSelect={open} />}
        </>
      ) : (
        <>
          {(timeline.status === "idle" || timeline.status === "loading") && (
            <LoadingMessage>Loading timeline…</LoadingMessage>
          )}
          {timeline.status === "error" && (
            <ErrorMessage
              context="Session timeline"
              error={timeline.error}
              retry={() => void loadTimeline()}
            />
          )}
          {timeline.status === "ok" && (
            <Fragment key={sessionId}>
              <TaskHeader session={timeline.data.session} />
              <SessionTimeline
                turns={timeline.data.turns}
                moveMatch={(direction) => moveTurn(workspace, direction, true)}
              />
            </Fragment>
          )}
        </>
      )}
    </aside>
  );
}
