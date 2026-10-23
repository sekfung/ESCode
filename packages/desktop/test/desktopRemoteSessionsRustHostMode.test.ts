import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("electron", () => ({
  BrowserWindow: { fromId: vi.fn() },
  MessageChannelMain: class MessageChannelMain {
    readonly port1 = { id: "port-1", close: vi.fn() };
    readonly port2 = { id: "port-2", close: vi.fn() };
  },
}));

import { HostMessageTypes } from "@escode/shared";
import { createRemoteWorkspaceSessionManager } from "../src/main/desktopRemoteSessions.js";

// docs/specs/rust-u2-window-host.md §5-1：Rust 模式下远程 workspace 走显式的 per-window
// Node Host（`ensureWindowHost`），不读取/写入本地窗口的 Rust attachment map。

describe("remote workspace host ownership in Rust mode", () => {
  afterEach(() => {
    vi.clearAllMocks();
  });

  it("lazily creates a per-window Node Host when the local map is empty", async () => {
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
      { kind: "ssh", host: "example.test", user: "demo" } as never,
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
});
