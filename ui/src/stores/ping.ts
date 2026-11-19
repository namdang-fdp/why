import { create } from "zustand";
import { ipc } from "@/lib/ipc";

type PingState =
  { status: "idle" } | { status: "ok"; version: string } | { status: "error"; message: string };

type PingStore = { ping: PingState; run: () => Promise<void> };

export const usePingStore = create<PingStore>((set) => ({
  ping: { status: "idle" },
  run: async () => {
    try {
      set({ ping: { status: "ok", version: await ipc.ping() } });
    } catch (e) {
      set({ ping: { status: "error", message: String(e) } });
    }
  },
}));
