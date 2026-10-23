import { beforeEach, describe, expect, it, vi } from "vitest";

const { quit, logTerminal } = vi.hoisted(() => ({
  quit: vi.fn(),
  logTerminal: vi.fn(),
}));
vi.mock("../src/main/databaseStartupRelay.js", () => ({
  requestDatabaseStartupQuit: quit,
  logDatabaseStartupTerminal: logTerminal,
}));

vi.mock("electron", () => ({
  ipcMain: {
    on: vi.fn(),
    removeListener: vi.fn(),
  },
  MessageChannelMain: class MessageChannelMain {
    readonly port1 = { id: "port-1" };
    readonly port2 = { id: "port-2" };
  },
}));

import { ipcMain } from "electron";
import {
  attachRustHostWindow,
  type RustWindowAttachment,
} from "../src/main/desktopRustHostWindow.js";

function fakeWindow() {
  const webContents = {
    id: 7,
    send: vi.fn(),
    postMessage: vi.fn(),
    isDestroyed: vi.fn(() => false),
  };
  const listeners = new Map<string, () => void>();
  const win = {
    webContents,
    isDestroyed: vi.fn(() => false),
    once: vi.fn((event: string, listener: () => void) => listeners.set(event, listener)),
    removeListener: vi.fn(),
  };
  return { win, listeners };
}

function fakeManager() {
  let lost: (() => void) | undefined;
  const attachment = {
    port: { id: "renderer-port" },
    dispose: vi.fn(),
    onLost: vi.fn((listener: () => void) => {
      lost = listener;
    }),
  };
  return {
    manager: {
      enabled: true,
      attachLocalPort: vi.fn(async () => attachment),
    },
    attachment,
    loseHost: () => lost?.(),
  };
}

describe("detached Rust Host window attachment", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  function rendererSnapshot(win: ReturnType<typeof fakeWindow>["win"]) {
    const entry = vi
      .mocked(ipcMain.on)
      .mock.calls.find(([channel]) => channel === "escode:database-startup-control");
    expect(entry).toBeDefined();
    const handler = entry![1] as (event: unknown, raw: unknown) => void;
    handler({ sender: win.webContents }, { action: "snapshot" });
  }

  it("waits for the renderer snapshot before posting the port", async () => {
    const { win } = fakeWindow();
    const { manager, attachment } = fakeManager();
    const result = await attachRustHostWindow({
      win: win as never,
      manager: manager as never,
      logger: { info: vi.fn(), warn: vi.fn() },
    });

    expect(manager.attachLocalPort).toHaveBeenCalledWith(
      expect.any(Function),
      "desktop-continuous",
      undefined,
    );
    // Rust Host ready 可能早于 renderer 注册 message 监听；snapshot 前不能投递。
    expect(win.webContents.postMessage).not.toHaveBeenCalled();
    expect(win.webContents.send).not.toHaveBeenCalled();
    expect(logTerminal).toHaveBeenCalledWith(expect.objectContaining({ phase: "ready" }));

    rendererSnapshot(win);

    expect(win.webContents.postMessage).toHaveBeenCalledWith(
      expect.any(String),
      { databaseStartupId: result.startupId },
      [attachment.port],
    );
    const state = win.webContents.send.mock.calls[0]?.[1] as Record<string, unknown>;
    expect(state.startupId).toBe(result.startupId);
    expect(state.phase).toBe("ready");
    result.dispose();
  });

  it("posts the port only once across repeated renderer snapshots", async () => {
    const { win } = fakeWindow();
    const { manager } = fakeManager();
    const result = await attachRustHostWindow({
      win: win as never,
      manager: manager as never,
      logger: { info: vi.fn(), warn: vi.fn() },
    });

    rendererSnapshot(win);
    rendererSnapshot(win);

    expect(win.webContents.postMessage).toHaveBeenCalledTimes(1);
    result.dispose();
  });

  it("keeps the attachment and reports a restarting startup on loss", async () => {
    const { win } = fakeWindow();
    const { manager, attachment, loseHost } = fakeManager();
    const result: RustWindowAttachment = await attachRustHostWindow({
      win: win as never,
      manager: manager as never,
      logger: { info: vi.fn(), warn: vi.fn() },
    });

    rendererSnapshot(win);
    loseHost();
    const restarting = win.webContents.send.mock.calls.at(-1)?.[1] as Record<string, unknown>;
    expect(restarting.phase).toBe("preparing_session_storage");
    expect(restarting.databasePhase).toBe("checking");
    expect(attachment.dispose).not.toHaveBeenCalled();

    result.dispose();
    expect(attachment.dispose).toHaveBeenCalledTimes(1);
    expect(ipcMain.removeListener).toHaveBeenCalled();
  });
});
