import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("electron", () => ({
  BrowserWindow: { fromId: vi.fn() },
  MessageChannelMain: class MessageChannelMain {
    readonly port1 = { id: "port-1", close: vi.fn() };
    readonly port2 = { id: "port-2", close: vi.fn() };
  },
}));

import { HostMessageTypes, HostResponseTypes } from "@escode/shared";
import { createRemoteWorkspaceSessionManager } from "../src/main/desktopRemoteSessions.js";

// S5-a（docs/specs/source-server-remote-rust-host.md §12.2）：ssh/wsl/docker 远程 workspace 已走
// Rust Host；per-window Node Host 只保留给显式 Web 服务端（`kind: "server"`）。

describe("remote workspace host ownership in Rust mode", () => {
  afterEach(() => {
    vi.clearAllMocks();
  });

  it("keeps the per-window Node Host path only for a Web server target", async () => {
    const child = {
      pid: 4321,
      on: vi.fn(),
      once: vi.fn(),
      removeListener: vi.fn(),
      postMessage: vi.fn(),
    };
    const ensureWindowHost = vi.fn(async () => child);
    const windowHostProcessMap = new Map<number, typeof child>();
    const manager = createRemoteWorkspaceSessionManager({
      logger: { info: vi.fn(), warn: vi.fn(), error: vi.fn() },
      windowHostProcessMap: windowHostProcessMap as never,
      ensureWindowHost: ensureWindowHost as never,
      resolveRemoteAssetDirs: () => ({}) as never,
    });
    const win = {
      webContents: {
        id: 7,
        send: vi.fn(),
        isDestroyed: vi.fn(() => false),
      },
      isDestroyed: vi.fn(() => false),
    };

    const pending = manager.createRemoteWorkspaceSession(
      win as never,
      { kind: "server", url: "https://example.test" } as never,
      "request-1",
    );
    let rejected: unknown;
    pending.catch((error) => {
      rejected = error;
    });

    await new Promise((resolve) => setTimeout(resolve, 100));
    if (rejected) throw rejected;
    expect(child.postMessage).toHaveBeenCalled();
    expect(ensureWindowHost).toHaveBeenCalledWith(win);
    expect(windowHostProcessMap.get(7)).toBe(child);
    expect(child.postMessage.mock.calls[0]?.[0]).toMatchObject({
      type: HostMessageTypes.ConnectRemoteWorkspace,
      requestId: "request-1",
    });

    manager.cancelPendingRemoteWorkspaceSessionsForWindow(7);
  });

  // S5-b（spec §12.2 / §12.5）：远程缺少 Rust Host resolver 时必须显式报错，不能回退 Node Host。
  it("rejects ssh with REMOTE_RUST_HOST_UNAVAILABLE instead of falling back to Node Host", async () => {
    const ensureWindowHost = vi.fn();
    const manager = createRemoteWorkspaceSessionManager({
      logger: { info: vi.fn(), warn: vi.fn(), error: vi.fn() },
      windowHostProcessMap: new Map() as never,
      ensureWindowHost: ensureWindowHost as never,
      resolveRemoteAssetDirs: () => ({}) as never,
    });
    const win = {
      webContents: {
        id: 8,
        send: vi.fn(),
        isDestroyed: vi.fn(() => false),
      },
      isDestroyed: vi.fn(() => false),
    };

    await expect(
      manager.createRemoteWorkspaceSession(
        win as never,
        { kind: "ssh", host: "example.test", port: 22, username: "dev" } as never,
        "request-no-rust",
      ),
    ).rejects.toMatchObject({ code: "REMOTE_RUST_HOST_UNAVAILABLE" });
    expect(ensureWindowHost).not.toHaveBeenCalled();
  });

  it("rejects a Node Host descriptor for a non-server workspace", async () => {
    const listeners = new Map<string, (message: unknown) => void>();
    const child = {
      pid: 4321,
      on: vi.fn((event: string, listener: (message: unknown) => void) => {
        listeners.set(event, listener);
      }),
      once: vi.fn(),
      removeListener: vi.fn(),
      postMessage: vi.fn(),
    };
    const windowHostProcessMap = new Map<number, typeof child>();
    const manager = createRemoteWorkspaceSessionManager({
      logger: { info: vi.fn(), warn: vi.fn(), error: vi.fn() },
      windowHostProcessMap: windowHostProcessMap as never,
      ensureWindowHost: (async () => child) as never,
      resolveRemoteAssetDirs: () => ({}) as never,
    });
    const win = {
      webContents: {
        id: 7,
        send: vi.fn(),
        isDestroyed: vi.fn(() => false),
      },
      isDestroyed: vi.fn(() => false),
    };

    const pending = manager.createRemoteWorkspaceSession(
      win as never,
      { kind: "server", url: "https://example.test" } as never,
      "request-rogue",
    );
    await vi.waitFor(() =>
      expect(child.postMessage).toHaveBeenCalledWith(
        expect.objectContaining({
          type: HostMessageTypes.ConnectRemoteWorkspace,
          requestId: "request-rogue",
        }),
      ),
    );

    listeners.get("message")?.({
      type: HostResponseTypes.RemoteWorkspaceConnected,
      requestId: "request-rogue",
      descriptor: {
        remoteSessionId: "remote-session-rogue",
        target: { kind: "ssh", host: "example.test", username: "dev" },
        workspacePath: "/srv/repo",
        workspaceIdentity: "remote:ssh:example.test:22:dev:/srv/repo",
        generation: 1,
      },
    });

    await expect(pending).rejects.toMatchObject({ code: "REMOTE_RUST_HOST_REQUIRED" });
    expect(child.postMessage).toHaveBeenCalledWith(
      expect.objectContaining({
        type: HostMessageTypes.DisposeRemoteWorkspaceSession,
        remoteSessionId: "remote-session-rogue",
      }),
    );
    expect(windowHostProcessMap.get(7)).toBe(child);
  });

  // S5-a cancel（spec §12.2）：首连解析阶段取消必须命中 Host 的 caller key，而不是等 session 建好。
  it("cancels an in-flight profile resolution and aborts the Host connect", async () => {
    const { manager, resolveCalls, cancelRustRemoteProfile } = createCancelHarness();
    const win = windowStub(7);
    const pending = manager.createRemoteWorkspaceSession(
      win as never,
      { kind: "ssh", host: "example.test", username: "dev" } as never,
      "request-cancel",
    );
    const rejection = pending.catch((error: unknown) => error);
    await vi.waitFor(() => expect(resolveCalls).toHaveLength(1));

    manager.cancelPendingRemoteWorkspaceSessionsForWindow(7, "用户取消", "request-cancel");

    await expect(rejection).resolves.toMatchObject({ code: "REMOTE_CONNECT_CANCELLED" });
    expect(resolveCalls[0]?.signal?.aborted).toBe(true);
    expect(cancelRustRemoteProfile).toHaveBeenCalledTimes(1);
    expect(cancelRustRemoteProfile).toHaveBeenCalledWith(resolveCalls[0]?.request.requestId);
  });

  // 共享同一 flight 的其它窗口不能被一个窗口的取消误伤；最后一个 caller 才中止 Host 连接。
  it("keeps a shared profile resolution alive until the last caller cancels", async () => {
    const { manager, resolveCalls, cancelRustRemoteProfile } = createCancelHarness();
    const first = manager.createRemoteWorkspaceSession(
      windowStub(7) as never,
      { kind: "ssh", host: "example.test", username: "dev" } as never,
      "request-a",
    );
    const second = manager.createRemoteWorkspaceSession(
      windowStub(8) as never,
      { kind: "ssh", host: "example.test", username: "dev" } as never,
      "request-b",
    );
    const firstRejection = first.catch((error: unknown) => error);
    const secondRejection = second.catch((error: unknown) => error);
    await vi.waitFor(() => expect(resolveCalls).toHaveLength(1));

    manager.cancelPendingRemoteWorkspaceSessionsForWindow(7, "用户取消", "request-a");
    await expect(firstRejection).resolves.toMatchObject({ code: "REMOTE_CONNECT_CANCELLED" });
    expect(cancelRustRemoteProfile).not.toHaveBeenCalled();
    expect(resolveCalls[0]?.signal?.aborted).toBe(false);

    manager.cancelPendingRemoteWorkspaceSessionsForWindow(8, "用户取消", "request-b");
    await expect(secondRejection).resolves.toMatchObject({ code: "REMOTE_CONNECT_CANCELLED" });
    expect(cancelRustRemoteProfile).toHaveBeenCalledTimes(1);
    expect(cancelRustRemoteProfile).toHaveBeenCalledWith(resolveCalls[0]?.request.requestId);
    expect(resolveCalls[0]?.signal?.aborted).toBe(true);
  });
});

function windowStub(webContentsId: number) {
  return {
    webContents: {
      id: webContentsId,
      send: vi.fn(),
      postMessage: vi.fn(),
      isDestroyed: vi.fn(() => false),
    },
    isDestroyed: vi.fn(() => false),
  };
}

/** 记录 resolver 请求并模拟 fetch：AbortSignal 中止时 reject。 */
function createCancelHarness() {
  const resolveCalls: Array<{ request: { requestId?: string }; signal?: AbortSignal }> = [];
  const resolveRustRemoteProfile = vi.fn(
    (request: { requestId?: string }, options?: { signal?: AbortSignal }) =>
      new Promise<never>((_resolve, reject) => {
        resolveCalls.push({ request, signal: options?.signal });
        options?.signal?.addEventListener("abort", () => {
          reject(Object.assign(new Error("The operation was aborted"), { name: "AbortError" }));
        });
      }),
  );
  const cancelRustRemoteProfile = vi.fn(async () => true);
  const manager = createRemoteWorkspaceSessionManager({
    logger: { info: vi.fn(), warn: vi.fn(), error: vi.fn() },
    windowHostProcessMap: new Map() as never,
    resolveRemoteAssetDirs: () => ({}) as never,
    resolveRustRemoteProfile,
    cancelRustRemoteProfile,
  });
  return { manager, resolveCalls, cancelRustRemoteProfile };
}
