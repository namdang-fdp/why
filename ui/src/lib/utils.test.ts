import { describe, expect, it } from "vitest";
import { cn } from "@/lib/utils";

describe("cn", () => {
  it("lets later Tailwind classes override earlier ones", () => {
    expect(cn("p-2", "p-4")).toBe("p-4");
  });
});
