// @vitest-environment jsdom
import { beforeEach, describe, expect, it } from "vitest";
import {
  armFirstSegmentProbe,
  markFirstSegmentFrame,
  markFirstSegmentRowApplied,
  readFirstSegmentProbe,
} from "../src/v4/firstSegmentProbe.js";

describe("firstSegmentProbe", () => {
  beforeEach(() => {
    armFirstSegmentProbe();
  });

  it("records the first frame and row only once", () => {
    markFirstSegmentFrame();
    const first = readFirstSegmentProbe();
    markFirstSegmentFrame();
    expect(readFirstSegmentProbe()?.firstFrameAt).toBe(first?.firstFrameAt);

    markFirstSegmentRowApplied();
    const withRow = readFirstSegmentProbe();
    markFirstSegmentRowApplied();
    expect(readFirstSegmentProbe()?.firstRowAt).toBe(withRow?.firstRowAt);
  });

  it("keeps timestamps monotonic and returns a copy", () => {
    markFirstSegmentFrame();
    markFirstSegmentRowApplied();
    const snapshot = readFirstSegmentProbe();
    expect(snapshot).not.toBeNull();
    expect(snapshot!.armedAt).toBeLessThanOrEqual(snapshot!.firstFrameAt);
    expect(snapshot!.firstFrameAt).toBeLessThanOrEqual(snapshot!.firstRowAt);

    snapshot!.firstRowAt = -1;
    expect(readFirstSegmentProbe()?.firstRowAt).not.toBe(-1);
  });
});
