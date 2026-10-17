import { afterEach, describe, expect, it, vi } from "vitest";
import { existsSync } from "node:fs";
import { mkdtemp } from "node:fs/promises";
import { createServer, type Server } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { RustHostStartError, startRustHost } from "../src/main/rustHostBridge.js";
import { createRustHostManager } from "../src/main/rustHostManager.js";
import { hostBinary } from "./rustHostTestSupport.js";

// Rust Host 启动失败（docs/specs/rust-u2-host-resilience.md §4）：四类各自带分类码、记 error、
// 在截止时间内失败（不挂起）。

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
      expect(manager.isRunning()).toBe(false);
    }
  });
});
