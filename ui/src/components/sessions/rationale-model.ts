import type {
  BlockPosition,
  Rationale,
  RationaleBlock,
  TimelineEventKind,
  TranscriptUnavailable,
} from "@/contracts";

/** What each reason means for a person. None of them is a failure of the agent or of WHY. */
export const unavailableText = {
  noTranscript: "No transcript was recorded for this session.",
  missing: "The transcript is no longer on disk.",
  outsideRoot: "The transcript is outside the folder WHY may read, so it was not opened.",
  unsupportedPlatform: "Reading transcripts is not supported on this platform yet.",
  notFound: "WHY could not find this turn or call in the transcript.",
  unreadable: "The transcript could not be read.",
  unsupportedHarness: "WHY cannot read transcripts for this harness yet.",
  notApplicable: "Not applicable to this kind of session.",
} satisfies Record<TranscriptUnavailable, string>;

export const positionText = {
  turn: "In this turn",
  before: "Just before",
  after: "Just after",
} satisfies Record<BlockPosition, string>;

/** The core caps a block's text and ends a cut block with `…`; an encrypted one is never an excerpt. */
export function isClipped(block: RationaleBlock) {
  return !block.encrypted && block.text.endsWith("…");
}

/** Collapse long text behind "Show more" (counted, not measured, so it never depends on layout). */
export const COLLAPSE_CHARS = 280;
export function isLong(text: string) {
  return text.length > COLLAPSE_CHARS || text.split("\n").length > 4;
}

/** The quiet caveats under the agent's words, in the order a reader needs them. */
export function caveats(rationale: Rationale) {
  const notes: string[] = [];
  if (!rationale.versionSupported) {
    const version = rationale.harnessVersion ? ` ${rationale.harnessVersion}` : "";
    notes.push(
      `The harness version${version} is newer than the ones WHY was checked against. The text is shown as recorded.`,
    );
  }
  if (rationale.truncated && rationale.blocks.length > 0) {
    notes.push("There may be more: the read stopped at a size limit.");
  }
  if (rationale.blocks.some(isClipped)) {
    notes.push("Some text is an excerpt, cut at the size limit.");
  }
  return notes;
}

/** Tool-class rows that can have a "Why?" (they carry the id the core looks the call up by). */
export function canExplainCall(kind: TimelineEventKind, toolUseId: string | null) {
  return toolUseId !== null && kind !== "prompt" && kind !== "assistantMessage" && kind !== "stop";
}
