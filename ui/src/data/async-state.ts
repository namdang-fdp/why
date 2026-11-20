import type { IpcError } from "@/contracts";

export type AsyncState<T> =
  | { status: "idle" }
  | { status: "loading" }
  | { status: "ok"; data: T }
  | { status: "error"; error: IpcError };

/** The two outcomes of a finished request. */
export type Settled<T> = Extract<AsyncState<T>, { status: "ok" | "error" }>;

export const idle = { status: "idle" } as const;

type IpcErrorKind = IpcError["kind"];
type KindWithMessage = Extract<IpcError, { message: string }>["kind"];

function withMessage<Kind extends KindWithMessage>(kind: Kind) {
  return (error: object) =>
    "message" in error && typeof error.message === "string"
      ? { kind, message: error.message }
      : undefined;
}

/** `untrustedFilters` carries the drivers that blocked the read; they are display data, passed through. */
function untrustedFilters(error: object) {
  return "drivers" in error && Array.isArray(error.drivers)
    ? ({ kind: "untrustedFilters", drivers: error.drivers } as Extract<
        IpcError,
        { kind: "untrustedFilters" }
      >)
    : undefined;
}

const readIpcError = {
  notARepo: () => ({ kind: "notARepo" }),
  gitMissing: () => ({ kind: "gitMissing" }),
  gitFailed: withMessage("gitFailed"),
  timeout: () => ({ kind: "timeout" }),
  notFound: () => ({ kind: "notFound" }),
  invalidInput: withMessage("invalidInput"),
  untrustedFilters,
  internal: withMessage("internal"),
} satisfies {
  [Kind in IpcErrorKind]: (error: object) => Extract<IpcError, { kind: Kind }> | undefined;
};

function isIpcErrorKind(kind: unknown): kind is IpcErrorKind {
  return typeof kind === "string" && Object.hasOwn(readIpcError, kind);
}

export function toIpcError(error: unknown): IpcError {
  if (
    typeof error === "object" &&
    error !== null &&
    "kind" in error &&
    isIpcErrorKind(error.kind)
  ) {
    const normalized = readIpcError[error.kind](error);
    if (normalized) return normalized;
  }
  // Not one of ours: the IPC layer itself failed (a command the capability
  // refuses, a closed channel, a thrown `Error`). That is not git's fault, so
  // it must not look like a git failure.
  return { kind: "internal", message: error instanceof Error ? error.message : String(error) };
}

export async function resolveState<T>(read: () => Promise<T>): Promise<Settled<T>> {
  try {
    return { status: "ok", data: await read() };
  } catch (error) {
    return { status: "error", error: toIpcError(error) };
  }
}
