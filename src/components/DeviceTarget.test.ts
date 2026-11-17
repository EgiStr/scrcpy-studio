import { describe, expect, it } from "vitest";
import { forTarget } from "./DeviceTarget";

const sets = [{ testset: "a.yaml" }, { testset: "b.yaml" }];

describe("forTarget", () => {
  it("leaves scenarios unpinned when no phone is picked", () => {
    expect(forTarget(sets, { serials: [], spread: "share", mode: "auto" })).toEqual(sets.map((s) => ({ ...s, mode: "auto" })));
  });

  it("shares scenarios across the picked phones", () => {
    const out = forTarget(sets, { serials: ["p1", "p2"], spread: "share", mode: "hidden" });
    expect(out).toHaveLength(2);
    expect(out.every((s) => s.mode === "hidden")).toBe(true);
    expect(out.every((s) => s.devices?.join() === "p1,p2")).toBe(true);
  });

  it("runs every scenario once on each picked phone", () => {
    const out = forTarget(sets, { serials: ["p1", "p2"], spread: "each", mode: "auto" });
    expect(out.map((s) => `${s.testset}@${s.devices?.join()}`)).toEqual(["a.yaml@p1", "b.yaml@p1", "a.yaml@p2", "b.yaml@p2"]);
  });
});
