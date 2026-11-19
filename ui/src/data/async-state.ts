import type { IpcError } from "@/contracts";

export type AsyncState<T> =
  | { status: "idle" }
  | { status: "loading" }
  | { status: "ok"; data: T }
  | { status: "error"; error: IpcError };

export const idle = { status: "idle" } as const;

export function toIpcError(error: unknown): IpcError {
  if (typeof error === "object" && error !== null && "kind" in error) {
    switch (error.kind) {
      case "notARepo":
      case "gitMissing":
      case "timeout":
      case "notFound":
        return { kind: error.kind };
      case "gitFailed":
      case "invalidInput":
        if ("message" in error && typeof error.message === "string") {
          return { kind: error.kind, message: error.message };
        }
        break;
    }
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
