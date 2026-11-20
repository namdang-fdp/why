import type { IpcError } from "@/contracts";

/** A short, human headline per kind; the technical `kind` is shown as secondary detail. */
export const errorTitles = {
  notARepo: "Not a Git repository",
  gitMissing: "Git isn't installed",
  gitFailed: "Git couldn't read this",
  timeout: "Git took too long",
  notFound: "Not found",
  invalidInput: "That isn't available",
  untrustedFilters: "Blocked until you review it",
  internal: "Something went wrong inside WHY",
} satisfies Record<IpcError["kind"], string>;

/** What each kind of core error means for a person (the `message` field, when there is one, is detail). */
export const errorMessages = {
  notARepo: "This location is not a Git repository.",
  gitMissing: "Git is unavailable. Check that Git is installed and on your PATH.",
  gitFailed: "Git could not complete the request.",
  timeout: "The request took too long.",
  notFound: "The requested repository, worktree, or file could not be found.",
  invalidInput: "This request is unavailable for the current selection.",
  untrustedFilters:
    "This repository sets up git filter programs. WHY has not read its files and has not run them. Review them first.",
  internal: "WHY could not complete the request because of an internal error.",
} satisfies Record<IpcError["kind"], string>;

/** One line for a toast or inline notice: the human sentence, then the detail. */
export function describeError(error: IpcError) {
  return "message" in error && error.message
    ? `${errorMessages[error.kind]} ${error.message}`
    : errorMessages[error.kind];
}

/**
 * Copy for the "add repository" flow. The core reports a bare repository, a
 * missing folder and a file as `invalidInput` with its own message, which reads
 * well on its own ("bare repositories are not supported"), so it is shown as
 * the headline instead of the generic "unavailable for the current selection".
 */
export function describeAddError(error: IpcError) {
  switch (error.kind) {
    case "notARepo":
      return "That folder is not inside a Git repository. Choose the repository's folder, or any folder within it.";
    case "gitMissing":
      return "Git was not found. Install Git and make sure it is on your PATH, then try again.";
    case "invalidInput":
      return sentence(error.message);
    case "timeout":
      return "Git took too long to answer. Is the folder on a slow or disconnected drive?";
    default:
      return describeError(error);
  }
}

/** For the base popover: git's own complaint ("`x` is not a commit") is the useful part. */
export function describeBaseError(error: IpcError) {
  return error.kind === "invalidInput" ? sentence(error.message) : describeError(error);
}

function sentence(text: string) {
  const trimmed = text.trim();
  if (!trimmed) return "This request was rejected.";
  return /[.!?]$/.test(trimmed)
    ? trimmed.charAt(0).toUpperCase() + trimmed.slice(1)
    : `${trimmed.charAt(0).toUpperCase()}${trimmed.slice(1)}.`;
}
