import { useContext, useEffect, useMemo } from "react";
import type { TimelineEvent, Turn, TurnRef } from "@/contracts";
import { locateInTimeline } from "@/components/diff/line-origins-model";
import { WorkspaceContext, useLineOrigins } from "@/stores/workspace-context";

export type TurnDetails =
  | { status: "loading" }
  | { status: "error" }
  /** The timeline is here; `turn` is `null` when it no longer holds that turn. */
  | { status: "ok"; turn: Turn | null; event: TimelineEvent | null };

/**
 * The prompt and the tool call behind a `TurnRef` (a line's origin). A `TurnRef`
 * only POINTS at a turn, so the words come from that session's timeline, fetched
 * the first time something asks (a hovered mark, an opened line) and cached.
 */
export function useTurnDetails(ref: TurnRef): TurnDetails {
  const workspace = useContext(WorkspaceContext);
  const timeline = useLineOrigins((state) => state.timelines.get(ref.sessionId));

  useEffect(() => {
    void workspace.getState().lineOrigins.getState().loadTimeline(ref.sessionId);
  }, [workspace, ref.sessionId]);

  return useMemo((): TurnDetails => {
    if (timeline?.status === "error") return { status: "error" };
    if (timeline?.status !== "ok") return { status: "loading" };
    return { status: "ok", ...locateInTimeline(timeline.data, ref) };
  }, [timeline, ref]);
}
