import { afterEach, describe, expect, it, vi } from "vitest";
import {
  forgetHostResourceUsage,
  requestExternalHostResourceUsage,
  resolveHostResourceUsageResult,
} from "../src/main/resourceManagerHostSampling.js";

describe("resource manager sampling lifetime", () => {
  afterEach(() => {
    forgetHostResourceUsage("test-host");
    vi.useRealTimers();
  });

  it("展示超时后复用在途采样，停止时取消且丢弃旧结果", async () => {
    vi.useFakeTimers();
    const send = vi.fn();
    const controller = new AbortController();
    const first = requestExternalHostResourceUsage("test-host", send, 900, controller.signal);
    const requestId = send.mock.calls[0][0].requestId;
    await vi.advanceTimersByTimeAsync(900);
    await expect(first).resolves.toEqual([]);
    const second = requestExternalHostResourceUsage("test-host", send, 900, controller.signal);
    await vi.advanceTimersByTimeAsync(900);
    await second;
    expect(send).toHaveBeenCalledTimes(1);
    controller.abort();
    expect(send).toHaveBeenLastCalledWith({
      type: "resource-usage-snapshot-cancel",
      requestId,
    });
    resolveHostResourceUsageResult("test-host", {
      type: "resource-usage-snapshot-result",
      requestId,
      sampledAt: 1,
      processes: [
        {
          pid: 123,
          name: "stale",
          category: "base",
          groupKey: "cli",
          groupLabel: "cli",
          cpuPercent: 1,
          memoryBytes: 1,
        },
      ],
    });
    const reopened = new AbortController();
    const next = requestExternalHostResourceUsage("test-host", send, 900, reopened.signal);
    await vi.advanceTimersByTimeAsync(900);
    await expect(next).resolves.toEqual([]);
    reopened.abort();
  });

  it("已经关闭的采样会话不发送请求", async () => {
    const send = vi.fn();
    const controller = new AbortController();
    controller.abort();
    await expect(
      requestExternalHostResourceUsage("test-host", send, 900, controller.signal),
    ).resolves.toEqual([]);
    expect(send).not.toHaveBeenCalled();
  });
});
