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

  it("posts the port with a synthetic ready state", async () => {
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
    );
    expect(win.webContents.postMessage).toHaveBeenCalledWith(
      expect.any(String),
      { databaseStartupId: result.startupId },
      [attachment.port],
    );
    const state = win.webContents.send.mock.calls[0]?.[1] as Record<string, unknown>;
    expect(state.startupId).toBe(result.startupId);
    expect(state.phase).toBe("ready");
    expect(logTerminal).toHaveBeenCalledWith(expect.objectContaining({ phase: "ready" }));
    result.dispose();
  });

  it("disposes only the attachment and reports a failed startup on loss", async () => {
    const { win } = fakeWindow();
    const { manager, attachment, loseHost } = fakeManager();
    const result: RustWindowAttachment = await attachRustHostWindow({
      win: win as never,
      manager: manager as never,
      logger: { info: vi.fn(), warn: vi.fn() },
    });

    loseHost();
    const failed = win.webContents.send.mock.calls.at(-1)?.[1] as Record<string, unknown>;
    expect(failed.phase).toBe("failed");
    expect(failed.errorCode).toBe("transport_closed");
    expect(attachment.dispose).not.toHaveBeenCalled();

    result.dispose();
    expect(attachment.dispose).toHaveBeenCalledTimes(1);
    expect(ipcMain.removeListener).toHaveBeenCalled();
  });
});
