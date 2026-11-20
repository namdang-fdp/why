import { createStore } from "zustand/vanilla";
import type { SessionId, SessionSummary, SessionTimeline, WorktreeId } from "@/contracts";
import { idle, resolveState, type AsyncState } from "@/data/async-state";
import type { DataSource } from "@/data/data-source";
import { sameJson } from "./same";

type SessionSource = Pick<DataSource, "listSessions" | "getSessionTimeline" | "onSessionsChanged">;

type SessionsState = {
  worktreeId: WorktreeId | null;
  list: AsyncState<SessionSummary[]>;
  sessionId: SessionId | null;
  timeline: AsyncState<SessionTimeline>;
  turnId: string | null;
  turnFocus: number;
  selectWorktree: (id: WorktreeId | null) => Promise<void>;
  selectSession: (id: SessionId | null) => Promise<void>;
  selectTurn: (id: string, focus?: boolean) => void;
  loadList: () => Promise<void>;
  loadTimeline: () => Promise<void>;
  watch: () => () => void;
};

function reuseTurns(previous: SessionTimeline, next: SessionTimeline) {
  const byId = new Map(previous.turns.map((turn) => [turn.id, turn]));
  const turns = next.turns.map((turn) => {
    const old = byId.get(turn.id);
    // Late tool results can update existing events without changing the count.
    return old && old.events.length === turn.events.length && sameJson(old, turn) ? old : turn;
  });
  return turns.length === previous.turns.length &&
    turns.every((turn, index) => turn === previous.turns[index])
    ? previous.turns
    : turns;
}

export function createSessionsStore(source: SessionSource, refreshWindowMs = 300) {
  return createStore<SessionsState>((set, get) => {
    let worktreeVersion = 0;
    let listVersion = 0;
    let timelineVersion = 0;

    async function loadList(background = false) {
      const { worktreeId, list } = get();
      if (worktreeId === null) return;
      const version = ++listVersion;
      if (!background || list.status !== "ok") set({ list: { status: "loading" } });

      const result = await resolveState(() => source.listSessions(worktreeId));
      if (version !== listVersion) return;
      if (result.status === "error") {
        if (background && get().list.status === "ok") {
          console.warn("Could not refresh the session list", result.error);
        } else {
          set({ list: result });
        }
        return;
      }
      const data = [...result.data].sort(
        (a, b) => Date.parse(b.lastEventAt) - Date.parse(a.lastEventAt),
      );
      const { sessionId } = get();
      if (sessionId !== null && !data.some(({ id }) => id === sessionId)) {
        void get().selectSession(null);
      }
      set({ list: { status: "ok", data } });
    }

    async function loadTimeline(background = false) {
      const { sessionId, timeline } = get();
      if (sessionId === null) return;
      const version = ++timelineVersion;
      if (!background || timeline.status !== "ok") set({ timeline: { status: "loading" } });

      const result = await resolveState(() => source.getSessionTimeline(sessionId));
      if (version !== timelineVersion) return;
      if (result.status === "error" && result.error.kind === "notFound") {
        const { list } = get();
        void get().selectSession(null);
        if (list.status === "ok") {
          set({ list: { status: "ok", data: list.data.filter(({ id }) => id !== sessionId) } });
        }
        return;
      }
      const { turnId, timeline: current } = get();
      if (result.status === "error") {
        if (background && current.status === "ok") {
          console.warn("Could not refresh the session timeline", result.error);
        } else {
          set({ timeline: result });
        }
        return;
      }
      const turns =
        current.status === "ok" ? reuseTurns(current.data, result.data) : result.data.turns;
      set({
        timeline: { status: "ok", data: { ...result.data, turns } },
        turnId: turns.some(({ id }) => id === turnId) ? turnId : null,
      });
    }

    return {
      worktreeId: null,
      list: idle,
      sessionId: null,
      timeline: idle,
      turnId: null,
      turnFocus: 0,
      selectWorktree: async (worktreeId) => {
        if (worktreeId === get().worktreeId) return;
        worktreeVersion++;
        listVersion++;
        timelineVersion++;
        set({ worktreeId, list: idle, sessionId: null, timeline: idle, turnId: null });
        await loadList();
      },
      selectSession: async (sessionId) => {
        if (sessionId === get().sessionId) return;
        timelineVersion++;
        set({ sessionId, timeline: idle, turnId: null });
        await loadTimeline();
      },
      selectTurn: (turnId, focus = false) =>
        set((state) => ({ turnId, turnFocus: state.turnFocus + Number(focus) })),
      loadList,
      loadTimeline,
      watch: () => {
        let stopped = false;
        let timer: ReturnType<typeof setTimeout> | undefined;
        let queued: number | undefined;
        let running: number | undefined;

        // One read pair per window for this selection. An event during a read
        // schedules a trailing refresh so no update is lost behind that read.
        function schedule() {
          if (stopped || timer !== undefined || queued === undefined || queued === running) return;
          timer = setTimeout(() => {
            timer = undefined;
            const version = queued;
            queued = undefined;
            if (version === undefined || version !== worktreeVersion) return;
            running = version;
            const session = timelineVersion;
            void loadList(true)
              .then(() => {
                if (!stopped && worktreeVersion === version && timelineVersion === session) {
                  return loadTimeline(true);
                }
              })
              .finally(() => {
                if (running === version) running = undefined;
                schedule();
              });
          }, refreshWindowMs);
        }

        const unlisten = source.onSessionsChanged(({ worktreeId }) => {
          if (worktreeId !== get().worktreeId) return;
          queued = worktreeVersion;
          schedule();
        });
        return () => {
          stopped = true;
          clearTimeout(timer);
          unlisten();
        };
      },
    };
  });
}
