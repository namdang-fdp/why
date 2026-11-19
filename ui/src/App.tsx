import { useEffect } from "react";
import { Activity } from "lucide-react";
import { Button } from "@/components/ui/button";
import { usePingStore } from "@/stores/ping";

export function App() {
  const ping = usePingStore((s) => s.ping);
  const run = usePingStore((s) => s.run);
  useEffect(() => {
    void run();
  }, [run]);

  return (
    <main className="flex h-screen flex-col items-center justify-center gap-4">
      <h1 className="text-6xl font-semibold tracking-tight">WHY</h1>
      <p className="text-muted-foreground">Because git diff tells you what, not why.</p>
      <p className="text-muted-foreground flex items-center gap-2 font-mono text-sm">
        <Activity className="size-4" aria-hidden />
        {ping.status === "idle" && "ping: …"}
        {ping.status === "ok" && `ping: core v${ping.version}`}
        {ping.status === "error" && `ping failed: ${ping.message}`}
      </p>
      <Button variant="outline" size="sm" onClick={() => void run()}>
        Ping again
      </Button>
    </main>
  );
}
