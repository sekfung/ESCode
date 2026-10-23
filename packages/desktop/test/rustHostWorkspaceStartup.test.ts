import { afterEach, describe, expect, it } from "vitest";
import { existsSync } from "node:fs";
import { mkdtemp, mkdir, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { createRustHostManager } from "../src/main/rustHostManager.js";
import type { HostPortLike } from "../src/main/rustHostBridge.js";

// docs/specs/rust-u2-window-host.md §5-7/§5-8：WorkspacePrepare 的真实 storageState 与
// 窗口级 env patch 经控制通道到达 Main；不再由 Main 合成 ready。

const hostBinary = resolve(
  `apps/escode-host/target/debug/escode-host${process.platform === "win32" ? ".exe" : ""}`,
);

describe("rust host workspace startup gate", () => {
  const managers: Array<ReturnType<typeof createRustHostManager>> = [];

  afterEach(async () => {
    await Promise.all(managers.splice(0).map((manager) => manager.stop()));
  });

  it.skipIf(!existsSync(hostBinary))(
    "reports the instance's real startup/storageState",
    async () => {
      const root = await mkdtemp(join(tmpdir(), "escode-rust-host-startup-"));
      const home = join(root, "home");
      const workspace = join(root, "workspace");
      await mkdir(home, { recursive: true });
      await mkdir(workspace, { recursive: true });
      const config = join(root, "model.json");
      await writeFile(
        config,
        JSON.stringify({
          providerId: "fixture",
          modelId: "fixture-model",
          reasoningLevel: "none",
          baseUrl: "http://127.0.0.1:9/v1",
        }),
      );
      const manager = createRustHostManager({
        binary: hostBinary,
        home,
        dataBaseDir: join(root, "data"),
        enabled: true,
      });
      managers.push(manager);

      const startup = new Promise<Record<string, unknown>>((resolveStartup) => {
        manager.onWorkspaceStartupState((event) => {
          const state = event.state as Record<string, unknown>;
          if (state.phase === "ready") resolveStartup(state);
        });
      });
      const [port1, port2] = createPortPair();
      await manager.attachLocalPort(() => ({ port1, port2 }), "desktop-continuous", {
        startupId: "startup-1",
        windowId: "window-1",
        workspace: { path: workspace, identity: workspace, config },
        runtimeEnvPatch: { ESCODE_WINDOW_PATCH: "1" },
      });

      const state = await startup;
      expect(state.phase).toBe("ready");
      expect(state.sessionId).toBeUndefined();
    },
    30_000,
  );
});

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
