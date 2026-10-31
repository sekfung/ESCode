import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("electron", () => ({
  BrowserWindow: { fromId: vi.fn(), getAllWindows: vi.fn(() => []) },
}));

import { createRemoteWorkspaceSessionManager } from "../src/main/desktopRemoteSessions.js";

// S5-a/S5-f-4（docs/specs/source-server-remote-rust-host.md §12.2/§12.6）：所有远程 workspace
// （含 `kind: "server"`）都由 App 级 Rust Host 承载；Main 不再创建 per-window Node Host。

const SERVER_TARGET = {
  kind: "server" as const,
  url: "https://example.test",
  token: "secret-token",
};

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

function loggerStub() {
  return { info: vi.fn(), warn: vi.fn(), error: vi.fn() };
}

/** resolver 成功返回 server Profile，并记录请求体供断言。 */
function createServerHarness(options?: {
  attachRustRemoteScope?: (scope: unknown) => Promise<{
    port: unknown;
    dispose(): void;
    onLost?(listener: (reason: unknown) => void): void;
    onClosed?(listener: (event: unknown) => void): void;
  }>;
}) {
  const resolveCalls: Array<{ target?: Record<string, unknown> }> = [];
  const resolveRustRemoteProfile = vi.fn(async (request: { target?: Record<string, unknown> }) => {
    resolveCalls.push(request);
    return {
      profileId: "11111111-1111-4111-8111-111111111111",
      sourceServerId: "22222222-2222-4222-8222-222222222222",
      target: {
        kind: "server",
        url: "https://example.test",
        tokenCredentialKey: "connection-profile:11111111-1111-4111-8111-111111111111:token",
      },
    };
  });
  const attachRustRemoteScope =
    options?.attachRustRemoteScope ??
    vi.fn(async () => ({ port: { id: "port" }, dispose: vi.fn() }));
  const manager = createRemoteWorkspaceSessionManager({
    logger: loggerStub(),
    resolveRustRemoteProfile,
    attachRustRemoteScope,
  });
  return { manager, resolveCalls, attachRustRemoteScope };
}

describe("remote workspace host ownership in Rust mode", () => {
  afterEach(() => {
    vi.clearAllMocks();
  });

  it("routes a server target through the Rust resolver instead of a window Node Host", async () => {
    const { manager, resolveCalls, attachRustRemoteScope } = createServerHarness();
    const win = windowStub(7);

    const sessionId = await (async () => {
      const pending = manager.createRemoteWorkspaceSession(
        win as never,
        SERVER_TARGET as never,
        "request-server",
      );
      await vi.waitFor(() => expect(win.webContents.postMessage).toHaveBeenCalled());
      const [, payload] = win.webContents.postMessage.mock.calls[0] as [
        string,
        { sessionId: string; attachmentId: string },
      ];
      manager.confirmRendererAttachmentReady(7, {
        sessionId: payload.sessionId,
        attachmentId: payload.attachmentId,
      });
      return pending;
    })();

    expect(resolveCalls[0]?.target).toMatchObject({
      kind: "server",
      url: "https://example.test",
      token: "secret-token",
    });
    expect(attachRustRemoteScope).toHaveBeenCalledWith({
      sourceServerId: "22222222-2222-4222-8222-222222222222",
      profileId: "11111111-1111-4111-8111-111111111111",
      workspaceIdentity: "remote:server:example.test:/",
    });
    expect(sessionId).toBeTruthy();
    expect(win.webContents.postMessage.mock.calls[0]?.[1]).toMatchObject({
      sessionId,
      target: {
        kind: "server",
        profileId: "11111111-1111-4111-8111-111111111111",
        sourceServerId: "22222222-2222-4222-8222-222222222222",
      },
    });
  });

  // S5-a/S5-f-4（spec §12.2 / §12.5）：缺少 Rust resolver 时必须显式报错，不能回退 Node Host。
  it("rejects a server target with REMOTE_RUST_HOST_UNAVAILABLE instead of falling back", async () => {
    const manager = createRemoteWorkspaceSessionManager({ logger: loggerStub() });
    const win = windowStub(8);

    await expect(
      manager.createRemoteWorkspaceSession(win as never, SERVER_TARGET as never, "request-no-rust"),
    ).rejects.toMatchObject({ code: "REMOTE_RUST_HOST_UNAVAILABLE" });
    expect(win.webContents.postMessage).not.toHaveBeenCalled();
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
    logger: loggerStub(),
    resolveRustRemoteProfile,
    cancelRustRemoteProfile,
  });
  return { manager, resolveCalls, cancelRustRemoteProfile };
}
