import { createServer, type IncomingMessage, type Server } from "node:http";
import { AddressInfo } from "node:net";
import { afterEach, describe, expect, it, vi } from "vitest";
import { WebSocket as WebSocketClient, WebSocketServer, type WebSocket as WsWebSocket } from "ws";

vi.mock("electron", () => ({
  BrowserWindow: { fromId: vi.fn(), getAllWindows: vi.fn(() => []) },
  MessageChannelMain: class MessageChannelMain {
    readonly port1 = { id: "port-1", close: vi.fn() };
    readonly port2 = { id: "port-2", close: vi.fn() };
  },
}));

import { BrowserWindow } from "electron";
import { HostMessageTypes, PlatformChannels } from "@escode/shared";
import { createRemoteWorkspaceSessionManager } from "../src/main/desktopRemoteSessions.js";
import {
  bridgeRustHostPort,
  issueRustHostCapability,
  resolveRustHostRemoteProfile,
  type RustHostHandle,
  type RustHostRemoteScope,
} from "../src/main/rustHostBridge.js";

// S3-c（docs/specs/source-server-remote-rust-host.md §10.12/§10.15）：Main 用 profile scope
// 申请 capability 并把 RPC WebSocket 桥接到 renderer；桌面远端窗口走 `/ws/host`
// （desktop-continuous），手机/Web 共享 Host attachment 走 `/ws`（replayable）。
// per-window Node Host 完全不参与这条路径。

const scope: RustHostRemoteScope = {
  sourceServerId: "33333333-3333-4333-8333-333333333333",
  profileId: "44444444-4444-4444-8444-444444444444",
  workspaceIdentity: "remote:ssh:host:/srv/repo",
};

interface FakeHost {
  handle: RustHostHandle;
  close(): Promise<void>;
  capabilityBodies: unknown[];
  profileBodies: unknown[];
  wsUpgrades: Array<{
    url: string | undefined;
    capability: string | undefined;
  }>;
  sockets: WsWebSocket[];
}

/** 最小假 Rust Host：`POST /api/rpc-host-capability` + `/ws`。 */
async function startFakeHost(options: {
  capabilityStatus?: number;
  capabilityError?: string;
  profileResolution?: { status?: number; body: unknown };
}): Promise<FakeHost> {
  const capabilityBodies: unknown[] = [];
  const profileBodies: unknown[] = [];
  const wsUpgrades: FakeHost["wsUpgrades"] = [];
  const sockets: WsWebSocket[] = [];
  const server: Server = createServer((request, response) => {
    const chunks: Buffer[] = [];
    request.on("data", (chunk: Buffer) => chunks.push(chunk));
    request.on("end", () => {
      if (
        request.url !== "/api/rpc-host-capability" &&
        request.url !== "/api/source-server-resolve-profile"
      ) {
        response.statusCode = 404;
        response.end();
        return;
      }
      expect(request.headers["x-escode-host-secret"]).toBe("secret");
      const body = Buffer.concat(chunks).toString("utf8");
      if (request.url === "/api/source-server-resolve-profile") {
        profileBodies.push(body ? JSON.parse(body) : undefined);
        response.setHeader("content-type", "application/json");
        response.statusCode = options.profileResolution?.status ?? 200;
        response.end(JSON.stringify(options.profileResolution?.body ?? {}));
        return;
      }
      capabilityBodies.push(body ? JSON.parse(body) : undefined);
      response.setHeader("content-type", "application/json");
      response.statusCode = options.capabilityStatus ?? 200;
      response.end(
        JSON.stringify(
          options.capabilityStatus && options.capabilityStatus >= 400
            ? { error: options.capabilityError ?? "refused" }
            : { capability: "cap-1" },
        ),
      );
    });
  });
  const wss = new WebSocketServer({ server });
  wss.on("connection", (socket: WsWebSocket, request: IncomingMessage) => {
    sockets.push(socket);
    wsUpgrades.push({
      url: request.url,
      capability: request.headers["x-escode-rpc-host-capability"] as string | undefined,
    });
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const { port } = server.address() as AddressInfo;
  const handle = {
    child: undefined,
    pid: undefined,
    addr: `127.0.0.1:${port}`,
    secret: "secret",
    send: vi.fn(),
    stop: vi.fn(async () => undefined),
    onExit: vi.fn(),
  } as unknown as RustHostHandle;
  return {
    handle,
    capabilityBodies,
    profileBodies,
    wsUpgrades,
    sockets,
    close: async () => {
      for (const socket of sockets) socket.terminate();
      await new Promise<void>((resolve) => wss.close(() => resolve()));
      await new Promise<void>((resolve) => server.close(() => resolve()));
    },
  };
}

function fakePort() {
  const listeners: Array<(event: unknown) => void> = [];
  const received: unknown[] = [];
  let closed = false;
  return {
    on: (_event: "message", listener: (event: unknown) => void) => listeners.push(listener),
    postMessage: (value: unknown) => received.push(value),
    start: () => undefined,
    close: () => {
      closed = true;
    },
    // 测试驱动：模拟 renderer 侧发出字节
    emit: (value: unknown) => {
      for (const listener of listeners) listener(value);
    },
    received,
    isClosed: () => closed,
  };
}

function frame(payload: Buffer): Buffer {
  const header = Buffer.alloc(13);
  header.writeUInt8(1, 0);
  header.writeUInt32BE(0, 1);
  header.writeUInt32BE(0, 5);
  header.writeUInt32BE(payload.length, 9);
  return Buffer.concat([header, payload]);
}

let running: FakeHost | undefined;
afterEach(async () => {
  await running?.close();
  running = undefined;
  vi.clearAllMocks();
});

describe("issueRustHostCapability with a remote scope", () => {
  it("posts the scope body and uses the process secret", async () => {
    running = await startFakeHost({});
    const capability = await issueRustHostCapability(running.handle, scope);
    expect(capability).toBe("cap-1");
    expect(running.capabilityBodies).toEqual([{ scope }]);
  });

  it("surfaces the Host error text without retrying or falling back", async () => {
    running = await startFakeHost({
      capabilityStatus: 400,
      capabilityError: "unknown source server profile: 44444444",
    });
    await expect(issueRustHostCapability(running.handle, scope)).rejects.toThrow(
      /remote workspace capability \(HTTP 400\): unknown source server profile/,
    );
    expect(running.capabilityBodies).toHaveLength(1);
  });
});

describe("resolveRustHostRemoteProfile", () => {
  it("posts the first-connect target and returns the backfilled scope", async () => {
    running = await startFakeHost({
      profileResolution: {
        body: {
          profileId: scope.profileId,
          sourceServerId: scope.sourceServerId,
          target: { kind: "ssh", host: "example.test", username: "dev" },
        },
      },
    });

    await expect(
      resolveRustHostRemoteProfile(running.handle, {
        target: { kind: "ssh", host: "example.test", username: "dev", password: "secret" },
      }),
    ).resolves.toEqual({
      profileId: scope.profileId,
      sourceServerId: scope.sourceServerId,
      target: { kind: "ssh", host: "example.test", username: "dev" },
    });
    expect(running.profileBodies).toEqual([
      {
        target: { kind: "ssh", host: "example.test", username: "dev", password: "secret" },
      },
    ]);
  });

  it("preserves structured identity-change errors", async () => {
    running = await startFakeHost({
      profileResolution: {
        status: 409,
        body: {
          error: "identity changed",
          code: "source-server-identity-change-pending",
          profileId: scope.profileId,
          pendingSourceServerId: "55555555-5555-4555-8555-555555555555",
          retryable: false,
        },
      },
    });

    await expect(
      resolveRustHostRemoteProfile(running.handle, { profileId: scope.profileId }),
    ).rejects.toMatchObject({
      code: "source-server-identity-change-pending",
      profileId: scope.profileId,
      pendingSourceServerId: "55555555-5555-4555-8555-555555555555",
      retryable: false,
    });
  });

  it("rejects a malformed successful response", async () => {
    running = await startFakeHost({
      profileResolution: { body: { profileId: scope.profileId } },
    });
    await expect(
      resolveRustHostRemoteProfile(running.handle, { profileId: scope.profileId }),
    ).rejects.toThrow(/malformed/);
  });
});

describe("bridgeRustHostPort with a remote scope", () => {
  it("uses /ws/host for a desktop continuous remote scope", async () => {
    running = await startFakeHost({});
    const port = fakePort();
    const teardown = bridgeRustHostPort(running.handle, port as never, "desktop-continuous", scope);
    try {
      await vi.waitFor(() => expect(running?.sockets).toHaveLength(1), {
        timeout: 5_000,
      });
      expect(running.capabilityBodies).toEqual([{ scope }]);
      expect(running.wsUpgrades[0]).toEqual({
        url: "/ws/host",
        capability: "cap-1",
      });
    } finally {
      teardown();
    }
  });

  it("bridges a scoped capability to /ws in both directions", async () => {
    running = await startFakeHost({});
    const port = fakePort();
    const teardown = bridgeRustHostPort(
      running.handle,
      port as never,
      "web-remote-replayable",
      scope,
    );
    try {
      await vi.waitFor(() => expect(running?.sockets).toHaveLength(1), {
        timeout: 5_000,
      });
      expect(running.capabilityBodies).toEqual([{ scope }]);
      expect(running.wsUpgrades[0]).toEqual({
        url: "/ws",
        capability: "cap-1",
      });

      const socket = running.sockets[0]!;
      // renderer → Host：payload 被包成 13 字节 Regular 帧。
      port.emit(Buffer.from([1, 2, 3]));
      const outbound = await new Promise<Buffer>((resolve) =>
        socket.once("message", (data) => resolve(Buffer.from(data as Buffer))),
      );
      expect(outbound.subarray(13)).toEqual(Buffer.from([1, 2, 3]));

      // Host → renderer：帧头被剥掉，只交 payload。
      socket.send(frame(Buffer.from([9, 8])));
      await vi.waitFor(() => expect(port.received).toContainEqual(Buffer.from([9, 8])));
      // teardown 关闭 socket；端口本身由调用方（manager/attachment）负责关闭。
      teardown();
      await vi.waitFor(() => expect(socket.readyState).not.toBe(WebSocketClient.OPEN));
    } finally {
      teardown();
    }
  });

  it("reports scope readiness only after the capability-issued socket opens", async () => {
    running = await startFakeHost({});
    const port = fakePort();
    const onReady = vi.fn();
    const teardown = bridgeRustHostPort(
      running.handle,
      port as never,
      "desktop-continuous",
      scope,
      { onReady },
    );
    try {
      expect(onReady).not.toHaveBeenCalled();
      await vi.waitFor(() => expect(onReady).toHaveBeenCalledTimes(1), { timeout: 5_000 });
      expect(running.sockets).toHaveLength(1);
    } finally {
      teardown();
    }
  });

  it("surfaces a refused capability as an explicit transport failure", async () => {
    running = await startFakeHost({
      capabilityStatus: 400,
      capabilityError: "unknown source server profile: 44444444",
    });
    const port = fakePort();
    const onReady = vi.fn();
    const onUnexpectedClose = vi.fn();
    const teardown = bridgeRustHostPort(
      running.handle,
      port as never,
      "desktop-continuous",
      scope,
      { onReady, onUnexpectedClose },
    );
    try {
      await vi.waitFor(() => expect(onUnexpectedClose).toHaveBeenCalledTimes(1), {
        timeout: 5_000,
      });
      expect(onUnexpectedClose.mock.calls[0]![0]).toMatchObject({
        reason: expect.stringContaining("HTTP 400"),
      });
      expect(onReady).not.toHaveBeenCalled();
    } finally {
      teardown();
    }
  });

  it("reports a Host-side close once and stays silent for explicit teardown", async () => {
    running = await startFakeHost({});
    const port = fakePort();
    const onUnexpectedClose = vi.fn();
    const teardown = bridgeRustHostPort(
      running.handle,
      port as never,
      "web-remote-replayable",
      scope,
      { onUnexpectedClose },
    );
    try {
      await vi.waitFor(() => expect(running?.sockets).toHaveLength(1), {
        timeout: 5_000,
      });
      // Host 主动结束 `/ws`（Source Server 链路 EOF，spec §10.14）。
      running.sockets[0]!.close(4401, "source server offline");
      await vi.waitFor(() => expect(onUnexpectedClose).toHaveBeenCalledTimes(1));
      expect(onUnexpectedClose.mock.calls[0]![0]).toMatchObject({ code: 4401 });

      const explicit = vi.fn();
      const secondPort = fakePort();
      const secondTeardown = bridgeRustHostPort(
        running.handle,
        secondPort as never,
        "web-remote-replayable",
        scope,
        { onUnexpectedClose: explicit },
      );
      await vi.waitFor(() => expect(running?.sockets).toHaveLength(2), {
        timeout: 5_000,
      });
      secondTeardown();
      await vi.waitFor(() =>
        expect(running?.sockets[1]!.readyState).not.toBe(WebSocketClient.OPEN),
      );
      expect(explicit).not.toHaveBeenCalled();
    } finally {
      teardown();
    }
  });
});

/** 与 `desktopRemoteSessionsRustHostMode.test.ts` 同形的窗口夹具。 */
function windowFixture(webContentsId: number) {
  // `webContents.postMessage(channel, message, transfer)`：channel 是 `ScopedServicePort` 常量。
  const posted: Array<{
    channel: unknown;
    message: unknown;
    ports: unknown[];
  }> = [];
  const win = {
    webContents: {
      id: webContentsId,
      send: vi.fn(),
      postMessage: (channel: unknown, message: unknown, ports?: unknown[]) =>
        posted.push({ channel, message, ports }),
      isDestroyed: vi.fn(() => false),
    },
    isDestroyed: vi.fn(() => false),
  };
  return { win, posted };
}

function managerFixture(
  attachRustRemoteScope: unknown,
  options: {
    rendererAttachmentReadyTimeoutMs?: number;
    resolveRustRemoteProfile?: unknown;
  } = {},
) {
  const windowHostProcessMap = new Map<number, unknown>();
  const manager = createRemoteWorkspaceSessionManager({
    logger: { info: vi.fn(), warn: vi.fn(), error: vi.fn() },
    windowHostProcessMap: windowHostProcessMap as never,
    attachRustRemoteScope: attachRustRemoteScope as never,
    resolveRemoteAssetDirs: () => ({}) as never,
    ...options,
  });
  return { manager, windowHostProcessMap };
}

const profileTarget = {
  kind: "ssh" as const,
  host: "example.test",
  username: "demo",
  sourceServerId: scope.sourceServerId,
  profileId: scope.profileId,
};

describe("remote workspace session owned by the Rust Host", () => {
  it("resolves a first-connect target through Rust and backfills the profile scope", async () => {
    const port = { id: "renderer-port", close: vi.fn() };
    const dispose = vi.fn();
    const attachRustRemoteScope = vi.fn(async () => ({ port, dispose }));
    const resolveRustRemoteProfile = vi.fn(async () => ({
      profileId: scope.profileId,
      sourceServerId: scope.sourceServerId,
      target: {
        kind: "ssh",
        host: "example.test",
        port: 22,
        username: "demo",
        passwordCredentialKey: `connection-profile:${scope.profileId}:password`,
      },
    }));
    const { manager, windowHostProcessMap } = managerFixture(attachRustRemoteScope, {
      resolveRustRemoteProfile,
    });
    const { win, posted } = windowFixture(7);

    const pending = manager.createRemoteWorkspaceSession(
      win as never,
      {
        kind: "ssh",
        host: "Example.TEST",
        username: "demo",
        password: "first-connect-secret",
      },
      "req-first-connect",
      { workspacePath: "/srv/repo" },
    );
    await vi.waitFor(() => expect(posted).toHaveLength(1), { timeout: 5_000 });
    const payload = posted[0]!.message as { attachmentId: string; sessionId: string };
    manager.confirmRendererAttachmentReady(7, payload);
    await expect(pending).resolves.toBe(payload.sessionId);

    expect(resolveRustRemoteProfile).toHaveBeenCalledTimes(1);
    expect(resolveRustRemoteProfile).toHaveBeenCalledWith(
      {
        target: expect.objectContaining({
          kind: "ssh",
          host: "Example.TEST",
          username: "demo",
          password: "first-connect-secret",
        }),
        // 首连解析按 caller key 取消（spec §12.2），Host 请求必须是独立 requestId。
        requestId: expect.any(String),
      },
      { signal: expect.any(AbortSignal) },
    );
    expect(attachRustRemoteScope).toHaveBeenCalledWith({
      profileId: scope.profileId,
      sourceServerId: scope.sourceServerId,
      workspaceIdentity: expect.stringContaining("/srv/repo"),
    });
    expect(posted[0]!.message).toMatchObject({
      sourceServerId: scope.sourceServerId,
      workspacePath: "/srv/repo",
      workspaceIdentity: expect.stringContaining("/srv/repo"),
      target: {
        kind: "ssh",
        host: "example.test",
        port: 22,
        username: "demo",
        profileId: scope.profileId,
        sourceServerId: scope.sourceServerId,
      },
    });
    expect(posted[0]!.message).not.toHaveProperty("target.password");
    expect(windowHostProcessMap.size).toBe(0);
  });

  it("single-flights concurrent first connect for the same target", async () => {
    const attachRustRemoteScope = vi.fn(async () => ({
      port: { id: `renderer-port-${attachRustRemoteScope.mock.calls.length}`, close: vi.fn() },
      dispose: vi.fn(),
    }));
    let release:
      | ((value: {
          profileId: string;
          sourceServerId: string;
          target: Record<string, unknown>;
        }) => void)
      | undefined;
    const resolveRustRemoteProfile = vi.fn(
      () =>
        new Promise<{
          profileId: string;
          sourceServerId: string;
          target: Record<string, unknown>;
        }>((resolve) => {
          release = resolve;
        }),
    );
    const { manager } = managerFixture(attachRustRemoteScope, { resolveRustRemoteProfile });
    const { win, posted } = windowFixture(7);
    const target = { kind: "ssh" as const, host: "example.test", username: "demo" };

    const first = manager.createRemoteWorkspaceSession(win as never, target, "req-a");
    const second = manager.createRemoteWorkspaceSession(win as never, target, "req-b");
    await vi.waitFor(() => expect(resolveRustRemoteProfile).toHaveBeenCalledTimes(1));
    release?.({
      profileId: scope.profileId,
      sourceServerId: scope.sourceServerId,
      target: { kind: "ssh", host: "example.test", port: 22, username: "demo" },
    });
    await vi.waitFor(() => expect(posted).toHaveLength(2), { timeout: 5_000 });
    for (const delivery of posted) {
      manager.confirmRendererAttachmentReady(
        7,
        delivery.message as { attachmentId: string; sessionId: string },
      );
    }
    await Promise.all([first, second]);

    expect(resolveRustRemoteProfile).toHaveBeenCalledTimes(1);
    expect(attachRustRemoteScope).toHaveBeenCalledTimes(2);
  });

  it("resolves a historical target that has profileId but no sourceServerId", async () => {
    const attachRustRemoteScope = vi.fn(async () => ({
      port: { id: "renderer-port", close: vi.fn() },
      dispose: vi.fn(),
    }));
    const resolveRustRemoteProfile = vi.fn(async () => ({
      profileId: scope.profileId,
      sourceServerId: scope.sourceServerId,
      target: { kind: "ssh", host: "example.test", port: 22, username: "demo" },
    }));
    const { manager } = managerFixture(attachRustRemoteScope, { resolveRustRemoteProfile });
    const { win, posted } = windowFixture(7);

    const pending = manager.createRemoteWorkspaceSession(
      win as never,
      { kind: "ssh", host: "example.test", username: "demo", profileId: scope.profileId },
      "req-history",
    );
    await vi.waitFor(() => expect(posted).toHaveLength(1), { timeout: 5_000 });
    manager.confirmRendererAttachmentReady(
      7,
      posted[0]!.message as { attachmentId: string; sessionId: string },
    );
    await pending;

    expect(resolveRustRemoteProfile).toHaveBeenCalledWith(
      { profileId: scope.profileId, requestId: expect.any(String) },
      { signal: expect.any(AbortSignal) },
    );
  });

  it("fails explicitly instead of falling back to Node when no resolver is wired", async () => {
    const { manager, windowHostProcessMap } = managerFixture(undefined);
    const { win } = windowFixture(7);
    await expect(
      manager.createRemoteWorkspaceSession(
        win as never,
        { kind: "ssh", host: "example.test", username: "demo" },
        "req-no-rust",
      ),
    ).rejects.toMatchObject({ code: "REMOTE_RUST_HOST_UNAVAILABLE" });
    expect(windowHostProcessMap.size).toBe(0);
  });

  it("creates and attaches a scoped session without touching the Node Host", async () => {
    const port = { id: "renderer-port", close: vi.fn() };
    const dispose = vi.fn();
    const attachRustRemoteScope = vi.fn(async () => ({ port, dispose }));
    const { manager, windowHostProcessMap } = managerFixture(attachRustRemoteScope);
    const { win, posted } = windowFixture(7);

    const pending = manager.createRemoteWorkspaceSession(win as never, profileTarget, "req-1", {
      workspacePath: "/srv/repo",
    });
    await vi.waitFor(() => expect(posted).toHaveLength(1), { timeout: 5_000 });
    const [delivery] = posted;
    const payload = delivery.message as {
      attachmentId: string;
      sessionId: string;
    };
    expect(attachRustRemoteScope).toHaveBeenCalledWith({
      sourceServerId: scope.sourceServerId,
      profileId: scope.profileId,
      workspaceIdentity: expect.stringContaining("/srv/repo"),
    });
    expect(delivery.ports).toEqual([port]);

    manager.confirmRendererAttachmentReady(7, {
      sessionId: payload.sessionId,
      attachmentId: payload.attachmentId,
    });
    await expect(pending).resolves.toBe(payload.sessionId);
    expect(windowHostProcessMap.size).toBe(0);
    expect(win.webContents.send).toHaveBeenCalled();

    manager.disposeRemoteWorkspaceSession(payload.sessionId);
    expect(dispose).toHaveBeenCalledTimes(1);
  });

  it("keeps two windows attached to the same remote workspace independent", async () => {
    const ports: Array<{ id: string; close: ReturnType<typeof vi.fn> }> = [];
    const dispose = vi.fn();
    const attachRustRemoteScope = vi.fn(async () => {
      const port = { id: `window-port-${ports.length}`, close: vi.fn() };
      ports.push(port);
      return { port, dispose };
    });
    const { manager, windowHostProcessMap } = managerFixture(attachRustRemoteScope);
    const first = windowFixture(7);
    const second = windowFixture(8);

    const firstPending = manager.createRemoteWorkspaceSession(
      first.win as never,
      profileTarget,
      "req-a",
      { workspacePath: "/srv/repo" },
    );
    const secondPending = manager.createRemoteWorkspaceSession(
      second.win as never,
      profileTarget,
      "req-b",
      { workspacePath: "/srv/repo" },
    );
    await vi.waitFor(
      () => expect(first.posted).toHaveLength(1) && expect(second.posted).toHaveLength(1),
      { timeout: 5_000 },
    );
    const firstAttachment = first.posted[0]!.message as {
      attachmentId: string;
      sessionId: string;
    };
    const secondAttachment = second.posted[0]!.message as {
      attachmentId: string;
      sessionId: string;
    };
    manager.confirmRendererAttachmentReady(7, firstAttachment);
    manager.confirmRendererAttachmentReady(8, secondAttachment);
    const [firstSessionId, secondSessionId] = await Promise.all([firstPending, secondPending]);

    // 同一远端 workspace 的多窗口各自持有 attachment；上游连接由 Host 按 profile 复用。
    expect(firstSessionId).not.toBe(secondSessionId);
    expect(ports.map((port) => port.id)).toEqual(["window-port-0", "window-port-1"]);
    expect(attachRustRemoteScope).toHaveBeenCalledTimes(2);
    expect(windowHostProcessMap.size).toBe(0);

    manager.disposeRemoteWorkspaceSession(firstSessionId);
    expect(dispose).toHaveBeenCalledTimes(1);
    manager.disposeRemoteWorkspaceSession(secondSessionId);
    expect(dispose).toHaveBeenCalledTimes(2);
  });

  it("reissues the scope after the attachment closes", async () => {
    const closedListeners: Array<(event: { code?: number; reasonText?: string }) => void> = [];
    const dispose = vi.fn();
    const attachRustRemoteScope = vi.fn(async () => {
      const attachment = {
        port: { id: `renderer-port-${closedListeners.length}`, close: vi.fn() },
        dispose,
        onLost: vi.fn(),
        onClosed: (listener: (event: { code?: number; reasonText?: string }) => void) =>
          closedListeners.push(listener),
      };
      return attachment;
    });
    const { manager, windowHostProcessMap } = managerFixture(attachRustRemoteScope);
    const { win, posted } = windowFixture(9);

    const firstPending = manager.createRemoteWorkspaceSession(
      win as never,
      profileTarget,
      "req-1",
      { workspacePath: "/srv/repo" },
    );
    await vi.waitFor(() => expect(posted).toHaveLength(1), { timeout: 5_000 });
    const first = posted[0]!.message as { attachmentId: string; sessionId: string };
    manager.confirmRendererAttachmentReady(9, first);
    const firstSessionId = await firstPending;
    expect(windowHostProcessMap.size).toBe(0);

    // Source Server 链路断开：Main 收口 route 并通知 renderer；renderer 的既有重连入口随后
    // 用同一 target/workspaceIdentity 重新 connectRemote（spec §10.14/§10.15）。
    closedListeners[0]!({ code: 4401, reasonText: "source server offline" });
    expect(win.webContents.send).toHaveBeenCalledWith(
      PlatformChannels.RemoteSessionClosed,
      expect.objectContaining({
        sessionId: firstSessionId,
        reasonCode: "source-server-offline",
        retryable: true,
      }),
    );
    expect(dispose).toHaveBeenCalledTimes(1);

    const secondPending = manager.createRemoteWorkspaceSession(
      win as never,
      profileTarget,
      "req-2",
      { workspacePath: "/srv/repo" },
    );
    await vi.waitFor(() => expect(posted).toHaveLength(2), { timeout: 5_000 });
    const second = posted[1]!.message as { attachmentId: string; sessionId: string };
    manager.confirmRendererAttachmentReady(9, second);
    const secondSessionId = await secondPending;
    expect(secondSessionId).not.toBe(firstSessionId);
    expect(attachRustRemoteScope).toHaveBeenCalledTimes(2);
    expect(attachRustRemoteScope).toHaveBeenLastCalledWith(
      expect.objectContaining({
        sourceServerId: scope.sourceServerId,
        profileId: scope.profileId,
        workspaceIdentity: expect.stringContaining("/srv/repo"),
      }),
    );
    expect(windowHostProcessMap.size).toBe(0);
  });

  it("reissues the scope on directory bind and disposes the previous attachment", async () => {
    const ports = [
      { id: "port-a", close: vi.fn() },
      { id: "port-b", close: vi.fn() },
    ];
    const dispose = vi.fn();
    const attachRustRemoteScope = vi.fn(async () => ({
      port: ports.shift(),
      dispose,
    }));
    const { manager } = managerFixture(attachRustRemoteScope);
    const { win, posted } = windowFixture(7);

    const pending = manager.createRemoteWorkspaceSession(win as never, profileTarget, "req-1", {
      workspacePath: "/srv/repo",
    });
    await vi.waitFor(() => expect(posted).toHaveLength(1), { timeout: 5_000 });
    const first = posted[0]!.message as {
      attachmentId: string;
      sessionId: string;
    };
    manager.confirmRendererAttachmentReady(7, first);
    const sessionId = await pending;

    vi.mocked(BrowserWindow.getAllWindows).mockReturnValue([win as never]);
    const binding = manager.bindRemoteWorkspaceSessionContext(
      sessionId,
      { workspacePath: "/srv/other" },
      7,
    );
    await vi.waitFor(() => expect(posted).toHaveLength(2), { timeout: 5_000 });
    const second = posted[1]!.message as {
      attachmentId: string;
      sessionId: string;
    };
    expect(second.sessionId).toBe(sessionId);
    expect(second.attachmentId).not.toBe(first.attachmentId);
    // bind 换代仍必须携带 Main 的权威 scope，Renderer 依赖它按 identity 找到 attachment。
    expect(second).toMatchObject({
      sourceServerId: scope.sourceServerId,
      workspacePath: "/srv/other",
      workspaceIdentity: `remote:server:${scope.sourceServerId}:/srv/other`,
    });

    manager.confirmRendererAttachmentReady(7, second);
    await expect(binding).resolves.toMatchObject({
      workspacePath: "/srv/other",
      // spec §5.1：Profile 连接的稳定身份必须回到 sourceServerId 前缀，重启/多窗口才能
      // 归回同一 Source 并触发启动重连。
      workspaceIdentity: `remote:server:${scope.sourceServerId}:/srv/other`,
    });
    // 候选就绪后才淘汰旧 attachment（换代期间 renderer 仍有可用 port）。
    expect(dispose).toHaveBeenCalledTimes(1);
    expect(attachRustRemoteScope).toHaveBeenLastCalledWith(
      expect.objectContaining({
        workspaceIdentity: expect.stringContaining("/srv/other"),
      }),
    );

    manager.disposeRemoteWorkspaceSession(sessionId);
    expect(dispose).toHaveBeenCalledTimes(2);
  });

  it("fails explicitly and leaves no route when the scoped capability is refused", async () => {
    const attachRustRemoteScope = vi.fn(async () => {
      throw new Error("Rust Host refused to issue a remote workspace capability (HTTP 400)");
    });
    const { manager, windowHostProcessMap } = managerFixture(attachRustRemoteScope);
    const { win, posted } = windowFixture(7);

    await expect(
      manager.createRemoteWorkspaceSession(win as never, profileTarget, "req-1"),
    ).rejects.toThrow(/remote workspace capability/);
    expect(posted).toHaveLength(0);
    expect(windowHostProcessMap.size).toBe(0);
    // 失败后没有残留 route：dispose 幂等且不抛。
    expect(() => manager.disposeRemoteWorkspaceSession("missing")).not.toThrow();
  });

  it("starts the renderer-ready timer only after the scope attachment is delivered", async () => {
    // 首次连接时 capability 申请包含安装（spec §11.3），交付端口前不能按 renderer 握手超时收口。
    let resolveAttach:
      | ((value: { port: { id: string; close: () => void }; dispose: () => void }) => void)
      | undefined;
    const attachRustRemoteScope = vi.fn(
      () =>
        new Promise<{ port: { id: string; close: () => void }; dispose: () => void }>((resolve) => {
          resolveAttach = resolve;
        }),
    );
    const { manager } = managerFixture(attachRustRemoteScope, {
      rendererAttachmentReadyTimeoutMs: 500,
    });
    const { win, posted } = windowFixture(7);

    const pending = manager.createRemoteWorkspaceSession(win as never, profileTarget, "req-1", {
      workspacePath: "/srv/repo",
    });
    await new Promise((resolve) => setTimeout(resolve, 60));
    expect(posted).toHaveLength(0);

    resolveAttach?.({ port: { id: "late-port", close: vi.fn() }, dispose: vi.fn() });
    await vi.waitFor(() => expect(posted).toHaveLength(1), { timeout: 5_000, interval: 5 });
    const payload = posted[0]!.message as { attachmentId: string; sessionId: string };
    manager.confirmRendererAttachmentReady(7, payload);
    await expect(pending).resolves.toBe(payload.sessionId);
  });

  it("rejects when the renderer never confirms after the port is delivered", async () => {
    const attachRustRemoteScope = vi.fn(async () => ({
      port: { id: "renderer-port", close: vi.fn() },
      dispose: vi.fn(),
    }));
    const { manager } = managerFixture(attachRustRemoteScope, {
      rendererAttachmentReadyTimeoutMs: 20,
    });
    const { win } = windowFixture(7);

    await expect(
      manager.createRemoteWorkspaceSession(win as never, profileTarget, "req-1", {
        workspacePath: "/srv/repo",
      }),
    ).rejects.toThrow(/renderer attachment ready 超时/);
  });

  it("rejects profile-scoped targets when the Host cannot serve remote scopes", async () => {
    const { manager } = managerFixture(undefined);
    const { win } = windowFixture(7);
    await expect(
      manager.createRemoteWorkspaceSession(win as never, profileTarget, "req-1"),
    ).rejects.toMatchObject({ code: "REMOTE_RUST_HOST_UNAVAILABLE" });
  });

  it("closes the route and notifies the renderer when the scope attachment is lost", async () => {
    const port = { id: "renderer-port", close: vi.fn() };
    const dispose = vi.fn();
    let closed: ((event: { code?: number }) => void) | undefined;
    const attachRustRemoteScope = vi.fn(async () => ({
      port,
      dispose,
      onClosed: (listener: typeof closed) => {
        closed = listener;
      },
    }));
    const { manager, windowHostProcessMap } = managerFixture(attachRustRemoteScope);
    const { win, posted } = windowFixture(7);

    const pending = manager.createRemoteWorkspaceSession(win as never, profileTarget, "req-1", {
      workspacePath: "/srv/repo",
    });
    await vi.waitFor(() => expect(posted).toHaveLength(1), { timeout: 5_000 });
    const payload = posted[0]!.message as {
      attachmentId: string;
      sessionId: string;
    };
    manager.confirmRendererAttachmentReady(7, payload);
    const sessionId = await pending;
    win.webContents.send.mockClear();

    expect(closed).toBeTypeOf("function");
    closed!({ code: 4401 });

    expect(win.webContents.send).toHaveBeenCalledWith(
      PlatformChannels.RemoteSessionClosed,
      expect.objectContaining({
        sessionId,
        reasonCode: "source-server-offline",
        retryable: true,
      }),
    );
    expect(dispose).toHaveBeenCalledTimes(1);
    expect(windowHostProcessMap.size).toBe(0);
    // 已收口的 route 不再持有 attachment；再次释放不应重复 dispose。
    manager.disposeRemoteWorkspaceSession(sessionId);
    expect(dispose).toHaveBeenCalledTimes(1);
  });
});

void HostMessageTypes;
