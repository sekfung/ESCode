import { afterEach, describe, expect, it, vi } from "vitest";
import { existsSync } from "node:fs";
import { mkdtemp, readFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { ChannelClient, MessagePortProtocol } from "@escode/rpc";
import { connectViaMessagePort } from "@escode/client";
import {
  bridgeRustHostPort,
  isRustHostEnabled,
  startRustHost,
  type HostPortLike,
  type RustHostHandle,
} from "../src/main/rustHostBridge.js";
import { createRustHostManager, defaultRustHostBinary } from "../src/main/rustHostManager.js";

// dev 开关（docs/specs/rust-u2-host-skeleton.md §6）的端到端验证，走**与 Main 相同的代码路径**：
// startRustHost（spawn + 控制握手）→ bridgeRustHostPort（MessagePort ↔ RPC WebSocket 字节桥）
// → 在端口另一侧用真实 `@escode/client` 调 settingService。Renderer 侧看到的仍是 MessagePort。

const hostBinary = resolve(
  `apps/escode-host/target/debug/escode-host${process.platform === "win32" ? ".exe" : ""}`,
);

describe("rust host dev switch", () => {
  let handle: RustHostHandle | undefined;

  afterEach(async () => {
    await handle?.stop();
    handle = undefined;
  });

  it("only enables on an explicit truthy flag", () => {
    expect(isRustHostEnabled({})).toBe(false);
    expect(isRustHostEnabled({ ESCODE_RUST_HOST: "0" })).toBe(false);
    expect(isRustHostEnabled({ ESCODE_RUST_HOST: "false" })).toBe(false);
    expect(isRustHostEnabled({ ESCODE_RUST_HOST: "1" })).toBe(true);
    expect(isRustHostEnabled({ ESCODE_RUST_HOST: " TRUE " })).toBe(true);
  });

  it.skipIf(!existsSync(hostBinary))(
    "bridges a window port to the rust host and serves the real client",
    async () => {
      const root = await mkdtemp(join(tmpdir(), "escode-rust-host-bridge-"));
      const home = join(root, "home");
      // dataBaseDir 经 rpcListen 下发给 Host（与 home 不同，见 rustHostRpc.test.ts 同理）。
      handle = await startRustHost({
        binary: hostBinary,
        home,
        dataBaseDir: join(root, "data-base"),
      });

      // Main 侧：renderer 拿到 port1（Electron `MessageChannelMain` 同形），Main 桥接 port2。
      // 测试用进程内端口对（vitest 下真实 `worker_threads` MessagePort 的投递不可靠；
      // 桥接逻辑与真实端口同形，Electron 分支只是这一层薄适配）。
      const [rendererPort, mainPort] = createPortPair();
      const teardown = bridgeRustHostPort(handle, mainPort, "desktop-continuous");

      const services = connectViaMessagePort(asMessagePort(rendererPort));
      // 交付模式由连接路径决定：continuous 桥 → hello 报 desktop-continuous。
      expect((await services.escodeAgentService.helloConversationV4()).clientMode).toBe(
        "desktop-continuous",
      );
      const initial = (await services.settingService.get()) as Record<string, unknown>;
      expect(initial).toBeTypeOf("object");

      await services.settingService.update({ locale: "zh-CN" } as never);
      const updated = (await services.settingService.get()) as Record<string, unknown>;
      expect(updated.locale).toBe("zh-CN");
      const onDisk = JSON.parse(
        await readFile(join(home, ".escode", "v2", "setting.json"), "utf8"),
      ) as Record<string, unknown>;
      expect(onDisk.locale).toBe("zh-CN", "写入落盘到 Main 指定的 home");

      // 未迁移 channel：显式缺口（Renderer 不挂起）。
      const [secondRenderer, secondMain] = createPortPair();
      const secondTeardown = bridgeRustHostPort(handle, secondMain, "web-remote-replayable");
      const client = new ChannelClient(new MessagePortProtocol(asMessagePort(secondRenderer)));
      await expect(client.getChannel("workspace").call("query", {})).rejects.toThrow(
        /does not implement channel `workspace` yet/,
      );

      teardown();
      secondTeardown();
    },
  );
});

describe("rust host control logs", () => {
  it.skipIf(!existsSync(hostBinary))(
    "delivers logBatch control frames to the Main log hook",
    async () => {
      const root = await mkdtemp(join(tmpdir(), "escode-rust-host-logs-"));
      const batches: { scope: string; level: string; lines: string[] }[] = [];
      const started = await startRustHost({
        binary: hostBinary,
        home: join(root, "home"),
        onLogBatch: (batch) => batches.push(batch),
      });
      try {
        // `shutdown` 是 Host 现成的结构化日志点（`main.rs`），用它验证整条链路：
        // Host 进程级 sink → 控制队列 → Main 的 `logBatch` 分派。
        started.child.stdin.write(`${JSON.stringify({ kind: "shutdown", reason: "test" })}\n`);
        await vi.waitFor(
          () => {
            expect(
              batches.some(
                (batch) =>
                  batch.scope === "host" &&
                  batch.level === "info" &&
                  batch.lines.some((line) => line.includes("shutdown: test")),
              ),
            ).toBe(true);
          },
          { timeout: 10_000 },
        );
      } finally {
        await started.stop();
      }
    },
    30_000,
  );
});

describe("rust host manager (dev switch)", () => {
  // 修复：与上面的端到端用例一致，需要已构建的 Host 二进制；单测作业不构建 Rust，缺失时跳过而不是超时失败。
  it.skipIf(!existsSync(hostBinary))(
    "gates on the flag and serves attachments through a window port",
    async () => {
      const root = await mkdtemp(join(tmpdir(), "escode-rust-host-manager-"));
      const home = join(root, "home");
      const manager = createRustHostManager({ binary: hostBinary, home, enabled: true });
      try {
        const attachment = await manager.attachLocalPort(() => {
          const [port1, port2] = createPortPair();
          return { port1, port2 };
        }, "web-remote-replayable");
        const services = connectViaMessagePort(asMessagePort(attachment.port as HostPortLike));
        // 远程 workspace共享 Host attachment 与 Node 路径同为 replayable（评审：A9 改动曾写死 continuous）。
        const hello = await services.escodeAgentService.helloConversationV4();
        expect(hello.clientMode).toBe("web-remote-replayable");
        expect(hello.deliveryProfile).toBe("replayable");
        await services.settingService.update({ locale: "en-US" } as never);
        const settings = (await services.settingService.get()) as Record<string, unknown>;
        expect(settings.locale).toBe("en-US");
        expect(manager.isRunning()).toBe(true);
        attachment.dispose();
      } finally {
        await manager.stop();
      }
      expect(manager.isRunning()).toBe(false);
    },
  );

  it("stays disabled unless the flag is on", () => {
    const manager = createRustHostManager({ binary: hostBinary, home: "/tmp/x", enabled: false });
    expect(manager.enabled).toBe(false);
  });

  it("resolves the packaged and dev binary paths", () => {
    const name = process.platform === "win32" ? "escode-host.exe" : "escode-host";
    expect(defaultRustHostBinary("/opt/escode/resources/app", true)).toBe(
      join("/opt/escode/resources", "escode-host", name),
    );
    expect(defaultRustHostBinary("/repo/packages/desktop", false)).toContain(
      join("apps", "escode-host", "target", "debug", name),
    );
    expect(
      defaultRustHostBinary("/repo/packages/desktop", false, {
        ESCODE_RUST_HOST_BINARY: "/custom/escode-host",
      }),
    ).toBe("/custom/escode-host");
  });
});

/** 进程内端口对：`postMessage` 立即投递给另一端（即 Electron MessageChannelMain 的语义）。 */
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

/**
 * Renderer 侧是 `MessagePortProtocol`（无帧）；桥接层负责与 Rust Host 的
 * SocketProtocol 13 字节帧互转，所以这里不能再套一层 SocketProtocol。
 */
function asMessagePort(port: HostPortLike) {
  const listeners: Array<(event: { data: unknown }) => void> = [];
  port.on("message", (event: { data?: unknown } | unknown) => {
    for (const listener of listeners) listener({ data: extract(event) });
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

function extract(event: { data?: unknown } | unknown): unknown {
  return event && typeof event === "object" && "data" in event
    ? (event as { data?: unknown }).data
    : event;
}
