import { invoke } from "@tauri-apps/api/core";

/** Typed wrappers around Tauri commands. Later replaced by ts-rs generated types. */
export const ipc = {
  /** Returns the app version from the Rust core. */
  ping: () => invoke<string>("ping"),
};
