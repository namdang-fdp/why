import { describe, expect, it } from "vitest";
import { minimalEdit, modelEdit, stripBom } from "./text-edit";

function apply(before: string, edit: ReturnType<typeof minimalEdit>) {
  return edit ? before.slice(0, edit.start) + edit.text + before.slice(edit.end) : before;
}

describe("minimalEdit", () => {
  it("returns null for equal text", () => {
    expect(minimalEdit("same", "same")).toBeNull();
  });

  it.each([
    ["line one\nline two\n", "line one\nline 2\nline three\n"],
    ["abc", ""],
    ["", "abc"],
    ["aaaa", "aaaaa"],
    ["abcabc", "abc"],
    ["x\ny\n", "x\ny\nz\n"],
    ["😀a", "😀b"],
    ["a😀", "b😀"],
  ])("produces an edit that turns %j into %j", (before, after) => {
    expect(apply(before, minimalEdit(before, after))).toBe(after);
  });

  it("touches only the differing middle", () => {
    expect(minimalEdit("keep A keep", "keep B keep")).toEqual({ start: 5, end: 6, text: "B" });
  });

  it("an append is a zero-width insertion at the end", () => {
    expect(minimalEdit("one\n", "one\ntwo\n")).toEqual({ start: 4, end: 4, text: "two\n" });
  });
});

describe("BOM handling (Monaco stores text without it)", () => {
  const bom = "\uFEFF";

  it("strips only a leading BOM", () => {
    expect(stripBom(`${bom}abc`)).toBe("abc");
    expect(stripBom(`a${bom}bc`)).toBe(`a${bom}bc`);
    expect(stripBom("")).toBe("");
  });

  it("sees no change when a BOM file is refreshed with the same content", () => {
    expect(modelEdit("line one\n", `${bom}line one\n`)).toBeNull();
  });

  it("never writes the BOM into the model, whatever the edit", () => {
    const model = "line one\n"; // as Monaco holds the file
    const incoming = `${bom}line one\nline two\n`;
    const edit = modelEdit(model, incoming);
    expect(edit).toEqual({ start: 9, end: 9, text: "line two\n" });
    expect(edit?.text).not.toContain(bom);
    // Applying it yields the same text as opening the changed file afresh.
    expect(model.slice(0, edit?.start) + edit?.text + model.slice(edit?.end)).toBe(
      stripBom(incoming),
    );
  });

  it("handles an edit on the first line of a BOM file", () => {
    expect(modelEdit("alpha\nbeta\n", `${bom}ALPHA\nbeta\n`)).toEqual({
      start: 0,
      end: 5,
      text: "ALPHA",
    });
  });
});
