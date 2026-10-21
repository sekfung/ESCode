import { afterEach, describe, expect, it, vi } from "vitest";
import { createMemoryDiagnosticsRegistry } from "@escode/shared";
import {
  readRendererHeapSnapshot,
  startMemoryDiagnosticsLogger,
} from "../src/lib/memoryDiagnostics.js";

describe("startMemoryDiagnosticsLogger", () => {
  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
    vi.unstubAllEnvs();
  });

  it("60s 采样：首次写盘，无变化不写，计数器变化再写；stop 后不再采样", () => {
    vi.useFakeTimers();
    const registry = createMemoryDiagnosticsRegistry();
    let cacheSize = 1;
    registry.register("shiki", () => ({ tokensCache: cacheSize }));
    const write = vi.fn();
    let nowMs = 0;
    const handle = startMemoryDiagnosticsLogger({
      registry,
      write,
      now: () => nowMs,
      readHeap: () => ({ usedJSHeapSize: 50 * 1024 * 1024, totalJSHeapSize: 80 * 1024 * 1024 }),
    });

    nowMs = 60_000;
    vi.advanceTimersByTime(60_000);
    expect(write).toHaveBeenCalledOnce();
    expect(write.mock.calls[0]?.[0]).toBe(
      "[memory] role=renderer reason=first heapUsedKb=51200 heapTotalKb=81920 shiki.tokensCache=1",
    );

    nowMs = 120_000;
    vi.advanceTimersByTime(60_000);
    expect(write).toHaveBeenCalledOnce();

    cacheSize = 2;
    nowMs = 180_000;
    vi.advanceTimersByTime(60_000);
    expect(write).toHaveBeenCalledTimes(2);
    expect(write.mock.calls[1]?.[0]).toContain("reason=changed");

    handle.stop();
    cacheSize = 3;
    nowMs = 240_000;
    vi.advanceTimersByTime(60_000);
    expect(write).toHaveBeenCalledTimes(2);
  });

  it("performance.memory 缺失时省略 heap 字段，仅按计数器门控", () => {
    const registry = createMemoryDiagnosticsRegistry();
    registry.register("xterm", () => ({ sessions: 0 }));
    const write = vi.fn();
    const handle = startMemoryDiagnosticsLogger({ registry, write, readHeap: () => undefined });
    expect(handle.sampleNow()).toBe(true);
    expect(write.mock.calls[0]?.[0]).toBe("[memory] role=renderer reason=first xterm.sessions=0");
    handle.stop();
  });

  it("provider 抛错只跳过自己；write 抛错只丢当前样本", () => {
    const registry = createMemoryDiagnosticsRegistry();
    registry.register("ok", () => ({ v: 1 }));
    registry.register("boom", () => {
      throw new Error("read failed");
    });
    const write = vi.fn(() => {
      throw new Error("bridge gone");
    });
    const handle = startMemoryDiagnosticsLogger({ registry, write, readHeap: () => undefined });
    expect(handle.sampleNow()).toBe(false);
    expect(write).toHaveBeenCalledOnce();
    expect(write.mock.calls[0]?.[0]).toContain("ok.v=1");
    handle.stop();
  });

  it("60s tick 写本地诊断日志", () => {
    vi.useFakeTimers();
    const registry = createMemoryDiagnosticsRegistry();
    registry.register("shiki", () => ({ tokensCache: 1 }));
    const write = vi.fn();
    const handle = startMemoryDiagnosticsLogger({
      registry,
      write,
      now: () => 60_000,
      readHeap: () => ({ usedJSHeapSize: 50 * 1024 * 1024, totalJSHeapSize: 80 * 1024 * 1024 }),
    });

    vi.advanceTimersByTime(60_000);

    expect(write.mock.calls[0]?.[0]).toContain("heapUsedKb=51200");
    handle.stop();
  });

  it("门控不写盘的 tick 不重复写日志", () => {
    const registry = createMemoryDiagnosticsRegistry();
    registry.register("shiki", () => ({ tokensCache: 1 }));
    const write = vi.fn();
    let nowMs = 0;
    const handle = startMemoryDiagnosticsLogger({
      registry,
      write,
      now: () => nowMs,
      readHeap: () => ({ usedJSHeapSize: 50 * 1024 * 1024 }),
    });

    expect(handle.sampleNow()).toBe(true);
    expect(write).toHaveBeenCalledOnce();

    nowMs = 60_000;
    expect(handle.sampleNow()).toBe(false);
    expect(write).toHaveBeenCalledOnce();
    handle.stop();
  });

  it("performance.memory 缺失时仍写本地日志", () => {
    const registry = createMemoryDiagnosticsRegistry();
    registry.register("xterm", () => ({ sessions: 0 }));
    const handle = startMemoryDiagnosticsLogger({
      registry,
      write: vi.fn(),
      readHeap: () => undefined,
    });

    expect(handle.sampleNow()).toBe(true);
    handle.stop();
  });

  it("计数器采集失败时跳过本次日志", () => {
    const handle = startMemoryDiagnosticsLogger({
      registry: {
        register: () => ({ dispose: () => {} }),
        collect: () => {
          throw new Error("counters unavailable");
        },
      },
      write: vi.fn(),
      readHeap: () => ({ usedJSHeapSize: 50 * 1024 * 1024 }),
    });

    expect(handle.sampleNow()).toBe(false);
    handle.stop();
  });

  it("readRendererHeapSnapshot 只在 Chromium performance.memory 存在时返回", () => {
    vi.stubGlobal("performance", {});
    expect(readRendererHeapSnapshot()).toBeUndefined();
    vi.stubGlobal("performance", { memory: { usedJSHeapSize: 10, totalJSHeapSize: 20 } });
    expect(readRendererHeapSnapshot()).toEqual({ usedJSHeapSize: 10, totalJSHeapSize: 20 });
  });
});

describe("logger.logMemoryDiagnostics", () => {
  afterEach(() => {
    vi.unstubAllEnvs();
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
    delete (globalThis as { __ESCODE_RENDERER_DISABLE_LOGGING__?: boolean })
      .__ESCODE_RENDERER_DISABLE_LOGGING__;
  });

  async function importLogger() {
    vi.resetModules();
    return import("../src/logger.js");
  }

  it("MEM-010 生产构建仍经桌面桥写 info", async () => {
    vi.stubEnv("NODE_ENV", "production");
    const bridgeLog = vi.fn();
    vi.stubGlobal("window", { escode: { log: bridgeLog } });
    const consoleLog = vi.spyOn(console, "log").mockImplementation(() => {});

    const { logMemoryDiagnostics } = await importLogger();
    logMemoryDiagnostics("[memory] role=renderer reason=first");

    expect(bridgeLog).toHaveBeenCalledWith("info", ["[memory] role=renderer reason=first"]);
    expect(consoleLog).not.toHaveBeenCalled();
  });

  it("无桌面桥（Web 端）且生产构建时 no-op", async () => {
    vi.stubEnv("NODE_ENV", "production");
    vi.stubGlobal("window", {});
    const consoleLog = vi.spyOn(console, "log").mockImplementation(() => {});

    const { logMemoryDiagnostics } = await importLogger();
    logMemoryDiagnostics("[memory] role=renderer reason=first");

    expect(consoleLog).not.toHaveBeenCalled();
  });

  it("全局禁用开关关闭时不输出", async () => {
    const bridgeLog = vi.fn();
    vi.stubGlobal("window", { escode: { log: bridgeLog } });
    (
      globalThis as { __ESCODE_RENDERER_DISABLE_LOGGING__?: boolean }
    ).__ESCODE_RENDERER_DISABLE_LOGGING__ = true;

    const { logMemoryDiagnostics } = await importLogger();
    logMemoryDiagnostics("[memory] role=renderer reason=first");

    expect(bridgeLog).not.toHaveBeenCalled();
  });
});
