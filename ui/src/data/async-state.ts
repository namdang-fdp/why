import type { IpcError } from "@/contracts";

export type AsyncState<T> =
  | { status: "idle" }
  | { status: "loading" }
  | { status: "ok"; data: T }
  | { status: "error"; error: IpcError };

export const idle = { status: "idle" } as const;

type IpcErrorKind = IpcError["kind"];
type KindWithMessage = Extract<IpcError, { message: string }>["kind"];

function withMessage<Kind extends KindWithMessage>(kind: Kind) {
  return (error: object) =>
    "message" in error && typeof error.message === "string"
      ? { kind, message: error.message }
      : undefined;
}

const readIpcError = {
  notARepo: () => ({ kind: "notARepo" }),
  gitMissing: () => ({ kind: "gitMissing" }),
  gitFailed: withMessage("gitFailed"),
  timeout: () => ({ kind: "timeout" }),
  notFound: () => ({ kind: "notFound" }),
  invalidInput: withMessage("invalidInput"),
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
  return { kind: "gitFailed", message: error instanceof Error ? error.message : String(error) };
}

export async function resolveState<T>(read: () => Promise<T>): Promise<AsyncState<T>> {
  try {
    return { status: "ok", data: await read() };
  } catch (error) {
    return { status: "error", error: toIpcError(error) };
  }
}
