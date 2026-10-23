import { afterEach, describe, expect, it, vi } from "vitest";
import { existsSync } from "node:fs";
import { mkdtemp } from "node:fs/promises";
import { createServer, type Server } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { connectViaMessagePort } from "@escode/client";
import {
  RustHostStartError,
  startRustHost,
  type HostPortLike,
  type RustHostHandle,
} from "../src/main/rustHostBridge.js";
import {
  createRustHostManager,
  type RustHostClock,
  type RustHostManager,
  type RustHostManagerState,
} from "../src/main/rustHostManager.js";
import { hostBinary } from "./rustHostTestSupport.js";

// Rust Host 韧性（docs/specs/rust-u2-host-resilience.md §3/§4）：
// - 四类启动失败各自带分类码、记 error、在截止时间内失败（不挂起）；
// - Host 被杀后受限自动重启、attachment onLost 后重建可用；连续崩溃超过上限停止并显式报错。
// 时序全部靠事件（onStateChange / onLost / 子进程 exit）推进；退避用注入时钟，不真实等待。

/** 用 Node 跑一段脚本冒充 Host 可执行文件（跨平台，不依赖 shell）。 */
function scriptHost(source: string) {
  return { binary: process.execPath, args: ["-e", source] };
}

const HOST_READY = `process.stdout.write(JSON.stringify({kind:"hostReady",protocolVersion:1,workspaces:0})+"\\n");`;

async function tempHome(prefix: string) {
  const root = await mkdtemp(join(tmpdir(), prefix));
  return join(root, "home");
}

function rejection(promise: Promise<unknown>): Promise<RustHostStartError> {
  return promise.then(
    () => {
      throw new Error("expected the Rust Host start to fail");
    },
    (error: unknown) => {
      expect(error).toBeInstanceOf(RustHostStartError);
      return error as RustHostStartError;
    },
  );
}

describe("rust host startup failures are classified and bounded", () => {
  // 截止时间远大于 vitest 的用例超时：二进制缺失 / 提前退出若退化成“等截止时间”，用例必然超时失败。
  const LONG_DEADLINE = 120_000;

  it("binary missing fails at spawn time", { timeout: 10_000 }, async () => {
    const home = await tempHome("escode-rust-host-missing-");
    const error = await rejection(
      startRustHost({
        binary: join(home, "no-such-escode-host"),
        home,
        timeoutMs: LONG_DEADLINE,
      }),
    );
    expect(error.code).toBe("RUST_HOST_BINARY_MISSING");
    expect(error.name).toBe("RustHostStartError");
    expect(error.details.errno).toBe("ENOENT");
  });

  it(
    "exiting before the handshake fails as soon as the process exits",
    { timeout: 10_000 },
    async () => {
      const home = await tempHome("escode-rust-host-exit-");
      const error = await rejection(
        startRustHost({ ...scriptHost("process.exit(3)"), home, timeoutMs: LONG_DEADLINE }),
      );
      expect(error.code).toBe("RUST_HOST_EXITED_EARLY");
      expect(error.details).toMatchObject({ stage: "hostReady", exitCode: 3, signal: null });
    },
  );

  it("exiting after hostReady names the rpcListening stage", { timeout: 10_000 }, async () => {
    const home = await tempHome("escode-rust-host-exit2-");
    const error = await rejection(
      startRustHost({
        // 读到 rpcListen 才退出：失败一定发生在第二阶段。
        ...scriptHost(`${HOST_READY} process.stdin.once("data", () => process.exit(4));`),
        home,
        timeoutMs: LONG_DEADLINE,
      }),
    );
    expect(error.code).toBe("RUST_HOST_EXITED_EARLY");
    expect(error.details).toMatchObject({ stage: "rpcListening", exitCode: 4 });
  });

  it("a silent host times out at the deadline as its own class", { timeout: 10_000 }, async () => {
    const home = await tempHome("escode-rust-host-timeout-");
    const error = await rejection(
      startRustHost({
        ...scriptHost("setInterval(() => {}, 1000)"),
        home,
        timeoutMs: 300,
      }),
    );
    expect(error.code).toBe("RUST_HOST_HANDSHAKE_TIMEOUT");
    expect(error.details).toEqual({ stage: "hostReady", timeoutMs: 300 });
  });

  it("the deadline covers the whole handshake, not each step", { timeout: 10_000 }, async () => {
    const home = await tempHome("escode-rust-host-timeout2-");
    const error = await rejection(
      startRustHost({
        ...scriptHost(`${HOST_READY} setInterval(() => {}, 1000)`),
        home,
        timeoutMs: 300,
      }),
    );
    expect(error.code).toBe("RUST_HOST_HANDSHAKE_TIMEOUT");
    expect(error.details).toEqual({ stage: "rpcListening", timeoutMs: 300 });
  });

  describe.skipIf(!existsSync(hostBinary))("listen failure (real host)", () => {
    let occupied: Server | undefined;
    afterEach(async () => {
      await new Promise<void>((done) => (occupied ? occupied.close(() => done()) : done()));
      occupied = undefined;
    });

    it("an occupied port fails with the listen class", { timeout: 30_000 }, async () => {
      occupied = createServer();
      const port = await new Promise<number>((done) =>
        occupied!.listen(0, "127.0.0.1", () => {
          const address = occupied!.address();
          done(typeof address === "object" && address ? address.port : 0);
        }),
      );
      const home = await tempHome("escode-rust-host-listen-");
      const error = await rejection(
        startRustHost({ binary: hostBinary, home, bind: `127.0.0.1:${port}` }),
      );
      expect(error.code).toBe("RUST_HOST_LISTEN_FAILED");
      expect(error.message).toMatch(/rpc listen failed/);
    });
  });

  it("the manager logs each class at error level and rethrows it unchanged", async () => {
    const cases: Array<{ code: string; host: { binary: string; args?: string[] } }> = [
      { code: "RUST_HOST_BINARY_MISSING", host: { binary: join(tmpdir(), "no-such-escode-host") } },
      { code: "RUST_HOST_EXITED_EARLY", host: scriptHost("process.exit(5)") },
      { code: "RUST_HOST_HANDSHAKE_TIMEOUT", host: scriptHost("setInterval(() => {}, 1000)") },
      {
        code: "RUST_HOST_LISTEN_FAILED",
        host: scriptHost(
          `${HOST_READY} process.stdin.once("data", () => process.stdout.write(JSON.stringify({kind:"failed",error:{code:"fatal",message:"rpc listen failed: address in use"}})+"\\n")); setInterval(() => {}, 1000);`,
        ),
      },
    ];
    for (const { code, host } of cases) {
      const log = { info: vi.fn(), warn: vi.fn(), error: vi.fn() };
      const manager = createRustHostManager({
        ...host,
        home: await tempHome("escode-rust-host-log-"),
        enabled: true,
        startTimeoutMs: 300,
        log,
      });
      const error = await rejection(manager.ensureStarted());
      expect(error.code, code).toBe(code);
      expect(log.error).toHaveBeenCalledWith(expect.stringContaining(`start failed (${code})`));
      expect(log.warn).not.toHaveBeenCalled();
      // 启动失败不占重启名额，也不进入 failed：下一次 ensureStarted 仍会重新尝试。
      expect(manager.state()).toEqual({ state: "idle" });
      expect(manager.isRunning()).toBe(false);
    }
  });
});

/** 受控时钟：记录每次退避的时长并立即触发（宏任务），`now` 由测试推进。 */
function manualClock(): RustHostClock & { delays: number[]; advance(ms: number): void } {
  let now = 1_000_000;
  const delays: number[] = [];
  return {
    delays,
    now: () => now,
    advance(ms) {
      now += ms;
    },
    setTimeout(callback, ms) {
      delays.push(ms);
      return setImmediate(callback);
    },
    clearTimeout(timer) {
      clearImmediate(timer as ReturnType<typeof setImmediate>);
    },
  };
}

/** 下一个满足条件的状态（事件驱动）。 */
function nextState(
  manager: RustHostManager,
  predicate: (state: RustHostManagerState) => boolean,
): Promise<RustHostManagerState> {
  return new Promise((done) => {
    const dispose = manager.onStateChange((state) => {
      if (predicate(state)) {
        dispose();
        done(state);
      }
    });
  });
}

/** 杀掉当前 Host 进程并等它真的退出（SIGKILL；Windows 上 Node 以 TerminateProcess 实现）。 */
async function killHost(handle: RustHostHandle) {
  const exited = new Promise<void>((done) => handle.onExit(() => done()));
  handle.child.kill("SIGKILL");
  await exited;
}

async function attachAndRead(manager: RustHostManager) {
  const attachment = await manager.attachLocalPort(() => {
    const [port1, port2] = createPortPair();
    return { port1, port2 };
  }, "web-remote-replayable");
  const services = connectViaMessagePort(asMessagePort(attachment.port as HostPortLike));
  const settings = (await services.settingService.get()) as Record<string, unknown>;
  expect(settings).toBeTypeOf("object");
  return attachment;
}

describe.skipIf(!existsSync(hostBinary))("rust host process crash", () => {
  let manager: RustHostManager | undefined;
  afterEach(async () => {
    await manager?.stop();
    manager = undefined;
  });

  it("restarts within the budget and attachments are rebuilt", { timeout: 60_000 }, async () => {
    const clock = manualClock();
    const log = { info: vi.fn(), warn: vi.fn(), error: vi.fn() };
    manager = createRustHostManager({
      binary: hostBinary,
      home: await tempHome("escode-rust-host-restart-"),
      enabled: true,
      clock,
      log,
    });
    const states: RustHostManagerState[] = [];
    manager.onStateChange((state) => states.push(state));

    const attachment = await attachAndRead(manager);
    const first = await manager.ensureStarted();
    const lost = new Promise<unknown>((done) => attachment.onLost(done));
    const restarted = nextState(manager, (state) => state.state === "running");

    await killHost(first);
    // 已打开的 attachment 立即丢失（端口已关闭），由消费方按 replayable 恢复重建。
    expect(await lost).toEqual({
      reason: "host-exited",
      exit: { code: null, signal: "SIGKILL" },
    });
    const running = await restarted;
    expect(running.state).toBe("running");
    const second = await manager.ensureStarted();
    expect(second.pid).not.toBe(first.pid);
    expect(clock.delays).toEqual([500]);
    expect(states.map((state) => state.state)).toEqual([
      "starting",
      "running",
      "restarting",
      "running",
    ]);
    expect(states[2]).toMatchObject({ attempt: 1, delayMs: 500 });
    expect(log.error).toHaveBeenCalledWith(
      expect.stringContaining("exited unexpectedly (code null, signal SIGKILL); restart 1/3"),
    );

    // 重建：新 attachment 接到新 Host，真实客户端可用。
    const rebuilt = await attachAndRead(manager);
    rebuilt.dispose();
  });

  it("serves multiple window attachments from one Host process", { timeout: 60_000 }, async () => {
    manager = createRustHostManager({
      binary: hostBinary,
      home: await tempHome("escode-rust-host-multi-attachment-"),
      enabled: true,
    });
    // 两个窗口的 attachment 共用同一个 Host：两次真实客户端调用都成功。
    const first = await attachAndRead(manager);
    const second = await attachAndRead(manager);
    const handle = await manager.ensureStarted();
    expect(handle.pid).toEqual(expect.any(Number));
    expect(manager.state().state).toBe("running");

    const firstLost = new Promise<unknown>((done) => first.onLost(done));
    const secondLost = new Promise<unknown>((done) => second.onLost(done));
    await killHost(handle);

    // Host 退出时两个 attachment 都必须收到丢失通知，由各自窗口重建。
    expect(await firstLost).toMatchObject({ reason: "host-exited" });
    expect(await secondLost).toMatchObject({ reason: "host-exited" });
    first.dispose();
    second.dispose();
  });

  it(
    "stops restarting after the budget and reports an explicit error",
    { timeout: 120_000 },
    async () => {
      const clock = manualClock();
      const log = { info: vi.fn(), warn: vi.fn(), error: vi.fn() };
      manager = createRustHostManager({
        binary: hostBinary,
        home: await tempHome("escode-rust-host-crash-loop-"),
        enabled: true,
        clock,
        log,
      });
      let handle = await manager.ensureStarted();
      // 60 s 窗口内连杀 3 次：每次都在上限内自动恢复。
      for (let attempt = 1; attempt <= 3; attempt += 1) {
        const restarted = nextState(manager, (state) => state.state === "running");
        await killHost(handle);
        await restarted;
        handle = await manager.ensureStarted();
        clock.advance(1_000);
      }
      expect(clock.delays).toEqual([500, 2_000, 5_000]);

      // 第 4 次：放弃，显式报错，不再拉起、不回退。
      const failed = nextState(manager, (state) => state.state === "failed");
      await killHost(handle);
      const state = await failed;
      expect(state).toMatchObject({ state: "failed", error: { code: "RUST_HOST_CRASH_LOOP" } });
      expect(clock.delays).toEqual([500, 2_000, 5_000]);
      expect(manager.isRunning()).toBe(false);
      await expect(manager.ensureStarted()).rejects.toMatchObject({
        name: "RustHostStartError",
        code: "RUST_HOST_CRASH_LOOP",
      });
      await expect(
        manager.attachLocalPort(() => {
          const [port1, port2] = createPortPair();
          return { port1, port2 };
        }, "web-remote-replayable"),
      ).rejects.toMatchObject({ code: "RUST_HOST_CRASH_LOOP" });
      expect(log.error).toHaveBeenCalledWith(expect.stringContaining("automatic restart stopped"));

      // 显式恢复入口：清空计数后重新拉起。
      manager.resetRestartBudget();
      expect(manager.state()).toEqual({ state: "idle" });
      await attachAndRead(manager);
    },
  );

  it("restarts that fall outside the window do not count", { timeout: 120_000 }, async () => {
    const clock = manualClock();
    manager = createRustHostManager({
      binary: hostBinary,
      home: await tempHome("escode-rust-host-window-"),
      enabled: true,
      clock,
    });
    let handle = await manager.ensureStarted();
    for (let round = 0; round < 4; round += 1) {
      const restarted = nextState(manager, (state) => state.state === "running");
      await killHost(handle);
      await restarted;
      handle = await manager.ensureStarted();
      // 每次之间隔 61 s：窗口内永远只有 1 次，退避始终是第 1 档。
      clock.advance(61_000);
    }
    expect(clock.delays).toEqual([500, 500, 500, 500]);
    expect(manager.state().state).toBe("running");
  });

  it("stop() is not an unexpected exit", { timeout: 30_000 }, async () => {
    const clock = manualClock();
    manager = createRustHostManager({
      binary: hostBinary,
      home: await tempHome("escode-rust-host-stop-"),
      enabled: true,
      clock,
    });
    await manager.ensureStarted();
    await manager.stop();
    expect(clock.delays).toEqual([]);
    expect(manager.state()).toEqual({ state: "idle" });
  });
});

/** 进程内端口对：`postMessage` 立即投递给另一端（Electron MessageChannelMain 语义）。 */
function createPortPair(): [HostPortLike, HostPortLike] {
  const listenersA: Array<(event: { data: unknown }) => void> = [];
  const listenersB: Array<(event: { data: unknown }) => void> = [];
  const portA: HostPortLike = {
    on: (_event, listener) => listenersA.push(listener as (event: { data: unknown }) => void),
    postMessage: (value) => {
      for (const listener of listenersB) listener({ data: value });
    },
    start: () => undefined,
    close: () => undefined,
  };
  const portB: HostPortLike = {
    on: (_event, listener) => listenersB.push(listener as (event: { data: unknown }) => void),
    postMessage: (value) => {
      for (const listener of listenersA) listener({ data: value });
    },
    start: () => undefined,
    close: () => undefined,
  };
  return [portA, portB];
}

/** Renderer 侧是 MessagePortProtocol（无帧）；桥接层负责 SocketProtocol 帧转换。 */
function asMessagePort(port: HostPortLike) {
  const listeners: Array<(event: { data: unknown }) => void> = [];
  port.on("message", (event: { data?: unknown } | unknown) => {
    const data =
      event && typeof event === "object" && "data" in event
        ? (event as { data?: unknown }).data
        : event;
    for (const listener of listeners) listener({ data });
  });
  port.start?.();
  return {
    addEventListener: (_type: "message", listener: (event: { data: unknown }) => void) => {
      listeners.push(listener);
    },
    removeEventListener: (_type: "message", listener: (event: { data: unknown }) => void) => {
      const index = listeners.indexOf(listener);
      if (index >= 0) listeners.splice(index, 1);
    },
    postMessage: (value: unknown) => port.postMessage(value),
    start: () => undefined,
    close: () => port.close?.(),
  } as never;
}
