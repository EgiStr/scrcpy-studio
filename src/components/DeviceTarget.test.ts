import { describe, expect, it } from "vitest";
import { forTarget } from "./DeviceTarget";

const sets = [{ testset: "a.yaml" }, { testset: "b.yaml" }];

describe("forTarget", () => {
  it("leaves scenarios unpinned when no phone is picked", () => {
    expect(forTarget(sets, { serials: [], spread: "share" })).toEqual(sets);
  });

  it("shares scenarios across the picked phones", () => {
    const out = forTarget(sets, { serials: ["p1", "p2"], spread: "share" });
    expect(out).toHaveLength(2);
    expect(out.every((s) => s.devices?.join() === "p1,p2")).toBe(true);
  });

  it("runs every scenario once on each picked phone", () => {
    const out = forTarget(sets, { serials: ["p1", "p2"], spread: "each" });
    expect(out.map((s) => `${s.testset}@${s.devices?.join()}`)).toEqual(["a.yaml@p1", "b.yaml@p1", "a.yaml@p2", "b.yaml@p2"]);
  });
});
