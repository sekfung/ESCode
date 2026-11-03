import { afterEach, describe, expect, it, vi } from "vitest";
vi.mock("../src/main/databaseStartupRelay.js", () => ({
  getDatabaseStartupPortPayload: () => ({ databaseStartupId: "window-host-test" }),
  requestDatabaseStartupQuit: vi.fn(),
  logDatabaseStartupTerminal: vi.fn(),
}));

vi.mock("electron", () => ({
  app: {
    setBadgeCount: vi.fn(),
    dock: undefined,
  },
  BrowserWindow: {
    fromId: vi.fn(),
  },
  Menu: {
    buildFromTemplate: vi.fn(() => ({ setMenu: vi.fn() })),
  },
  MessageChannelMain: class MessageChannelMain {
    readonly port1 = { id: "port-1" };
    readonly port2 = { id: "port-2" };
  },
  ipcMain: {
    on: vi.fn(),
    removeListener: vi.fn(),
  },
}));

vi.mock("../src/main/desktopWindowChrome.js", () => ({
  createBrowserWindow: vi.fn(),
}));

vi.mock("../src/main/armsBrowserPerfLoadNudge.js", () => ({
  scheduleArmsBrowserPerfLoadNudge: vi.fn(),
}));

import { handleDarwinWindowCloseRequest } from "../src/main/desktopDarwinCloseBehavior.js";
import { createBrowserWindow } from "../src/main/desktopWindowChrome.js";
import { ipcMain } from "electron";
import {
  createWindow,
  handleDesktopWindowCloseRequest,
} from "../src/main/desktopWindowLifecycle.js";

describe("desktopWindowLifecycle createWindow", () => {
  afterEach(() => {
    vi.useRealTimers();
  });

  it("Rust Host 模式下窗口只建 attachment，不 spawn utility process", async () => {
    let handleDomReady: (() => Promise<void>) | undefined;
    const webContents = {
      id: 48,
      on: vi.fn((event: string, listener: () => Promise<void>) => {
        if (event === "dom-ready") handleDomReady = listener;
      }),
      postMessage: vi.fn(),
      send: vi.fn(),
      isDestroyed: vi.fn(() => false),
    };
    const win = {
      id: 11,
      webContents,
      on: vi.fn(),
      once: vi.fn(),
      removeListener: vi.fn(),
      isDestroyed: vi.fn(() => false),
      show: vi.fn(),
      focus: vi.fn(),
    };
    vi.mocked(createBrowserWindow).mockReturnValue(win as never);
    const attachment = { port: { id: "rust-port" }, dispose: vi.fn(), onLost: vi.fn() };
    const startupListeners: Array<(state: Record<string, unknown>) => void> = [];
    const rustHostManager = {
      onWorkspaceStartupState: (listener: (state: Record<string, unknown>) => void) => {
        startupListeners.push(listener);
        return () => undefined;
      },
      attachLocalPort: vi.fn(async (_create, _mode, prepare) => {
        for (const listener of startupListeners) {
          listener({
            startupId: prepare.startupId,
            windowId: prepare.windowId,
            workspaceKey: prepare.workspace.path,
            generation: 1,
            state: { phase: "ready", sequence: 2 },
          });
        }
        return attachment;
      }),
      sendControl: vi.fn(),
      resetRestartBudget: vi.fn(),
    };

    createWindow({
      iconPath: "/tmp/icon.png",
      preloadPath: "/tmp/preload.js",
      logger: { info: vi.fn(), warn: vi.fn() },
      forceQuitRef: { current: false },
      rustHostManager: rustHostManager as never,
      syncAutoUpdaterStateToWindow: vi.fn(),
      syncReadyUpdateToWindow: vi.fn(),
      syncPostUpdateReleaseNotesToWindow: vi.fn(),
      disposeRemoteWorkspaceSessionsForWindow: vi.fn(),
      reattachRemoteWorkspaceSessionsForWindow: vi.fn(),
      agentWarmupTargets: [{ workspacePath: "/w", workspaceIdentity: "id" }],
      deviceMid: "device-mid",
      runtimeProcessEnvFallbackPatch: {},
    });

    await handleDomReady?.();

    expect(rustHostManager.attachLocalPort).toHaveBeenCalledWith(
      expect.any(Function),
      "desktop-continuous",
      expect.objectContaining({
        startupId: expect.any(String),
        windowId: "local-48",
        workspace: { path: "/w", identity: "id" },
        runtimeEnvPatch: {},
      }),
    );
    // renderer 注册 message 监听后发送 snapshot，Main 才投递 state/port。
    const snapshotHandler = vi
      .mocked(ipcMain.on)
      .mock.calls.filter(([channel]) => channel === "escode:database-startup-control")
      .at(-1)?.[1] as ((event: unknown, raw: unknown) => void) | undefined;
    expect(snapshotHandler).toBeDefined();
    snapshotHandler!({ sender: webContents }, { action: "snapshot" });

    expect(webContents.postMessage).toHaveBeenCalledWith(
      expect.any(String),
      expect.objectContaining({ databaseStartupId: expect.any(String) }),
      [attachment.port],
    );
    expect(webContents.send).toHaveBeenCalledWith(
      expect.any(String),
      expect.objectContaining({ phase: "ready" }),
    );
  });
});

describe("desktopWindowLifecycle handleDarwinWindowCloseRequest", () => {
  it("全屏时点击关闭应只退出全屏", () => {
    const logger = {
      info: vi.fn(),
    };
    const win = {
      isFullScreen: vi.fn(() => true),
      setFullScreen: vi.fn(),
      hide: vi.fn(),
    };

    const prevented = handleDarwinWindowCloseRequest({
      win,
      forceQuit: false,
      label: "local-1",
      logger,
    });

    expect(prevented).toBe(true);
    expect(win.setFullScreen).toHaveBeenCalledTimes(1);
    expect(win.setFullScreen).toHaveBeenCalledWith(false);
    expect(win.hide).not.toHaveBeenCalled();
    expect(logger.info).toHaveBeenCalledWith(
      "[createWindow] fullscreen close converted to leave-full-screen (local-1)",
    );
  });

  it("非全屏时点击关闭仍然隐藏窗口", () => {
    const logger = {
      info: vi.fn(),
    };
    const win = {
      isFullScreen: vi.fn(() => false),
      setFullScreen: vi.fn(),
      hide: vi.fn(),
    };

    const prevented = handleDarwinWindowCloseRequest({
      win,
      forceQuit: false,
      label: "local-2",
      logger,
    });

    expect(prevented).toBe(true);
    expect(win.hide).toHaveBeenCalledTimes(1);
    expect(win.setFullScreen).not.toHaveBeenCalled();
    expect(logger.info).toHaveBeenCalledWith(
      "[createWindow] window hidden instead of closed (local-2)",
    );
  });

  it("应用主动退出时不拦截关闭", () => {
    const logger = {
      info: vi.fn(),
    };
    const win = {
      isFullScreen: vi.fn(() => true),
      setFullScreen: vi.fn(),
      hide: vi.fn(),
    };

    const prevented = handleDarwinWindowCloseRequest({
      win,
      forceQuit: true,
      label: "local-3",
      logger,
    });

    expect(prevented).toBe(false);
    expect(win.hide).not.toHaveBeenCalled();
    expect(win.setFullScreen).not.toHaveBeenCalled();
    expect(logger.info).not.toHaveBeenCalled();
  });
});

describe("desktopWindowLifecycle handleDesktopWindowCloseRequest", () => {
  it("prevents closing when the last Windows window cancel is chosen", () => {
    const logger = {
      info: vi.fn(),
    };
    const confirmQuit = vi.fn(() => false);
    const requestQuit = vi.fn();

    const prevented = handleDesktopWindowCloseRequest({
      platform: "win32",
      forceQuit: false,
      closeToTraySupported: true,
      isLastWindow: true,
      label: "local-4",
      logger,
      confirmQuit,
      requestQuit,
    });

    expect(prevented).toBe(true);
    expect(confirmQuit).toHaveBeenCalledTimes(1);
    expect(requestQuit).not.toHaveBeenCalled();
    expect(logger.info).toHaveBeenCalledWith(
      "[createWindow] last window close canceled by user (local-4)",
    );
  });

  it("requests app quit when the last Windows window close is confirmed", () => {
    const logger = {
      info: vi.fn(),
    };
    const confirmQuit = vi.fn(() => true);
    const requestQuit = vi.fn();

    const prevented = handleDesktopWindowCloseRequest({
      platform: "win32",
      forceQuit: false,
      closeToTraySupported: true,
      isLastWindow: true,
      label: "local-5",
      logger,
      confirmQuit,
      requestQuit,
    });

    expect(prevented).toBe(true);
    expect(confirmQuit).toHaveBeenCalledTimes(1);
    expect(requestQuit).toHaveBeenCalledTimes(1);
    expect(logger.info).toHaveBeenCalledWith(
      "[createWindow] last window close confirmed, quitting app (local-5)",
    );
  });

  it("skips quit confirmation when confirmation is disabled", () => {
    const logger = {
      info: vi.fn(),
    };
    const confirmQuit = vi.fn(() => false);
    const requestQuit = vi.fn();

    const prevented = handleDesktopWindowCloseRequest({
      platform: "win32",
      forceQuit: false,
      closeToTraySupported: true,
      isLastWindow: true,
      label: "local-7",
      logger,
      shouldConfirmQuit: false,
      confirmQuit,
      requestQuit,
    });

    expect(prevented).toBe(true);
    expect(confirmQuit).not.toHaveBeenCalled();
    expect(requestQuit).toHaveBeenCalledTimes(1);
    expect(logger.info).toHaveBeenCalledWith(
      "[createWindow] last window close skipped confirmation, quitting app (local-7)",
    );
  });

  it("does not confirm quit when another window remains open", () => {
    const logger = {
      info: vi.fn(),
    };
    const confirmQuit = vi.fn(() => true);
    const requestQuit = vi.fn();

    const prevented = handleDesktopWindowCloseRequest({
      platform: "win32",
      forceQuit: false,
      closeToTraySupported: true,
      isLastWindow: false,
      label: "local-6",
      logger,
      confirmQuit,
      requestQuit,
    });

    expect(prevented).toBe(false);
    expect(confirmQuit).not.toHaveBeenCalled();
    expect(requestQuit).not.toHaveBeenCalled();
  });

  it("hides linux window to tray when tray is supported and close-to-tray enabled", () => {
    const logger = { info: vi.fn() };
    const hideWindow = vi.fn();
    const confirmQuit = vi.fn(() => true);
    const requestQuit = vi.fn();

    const prevented = handleDesktopWindowCloseRequest({
      platform: "linux",
      forceQuit: false,
      closeToTray: true,
      closeToTraySupported: true,
      isLastWindow: true,
      label: "local-linux-tray",
      logger,
      confirmQuit,
      requestQuit,
      hideWindow,
    });

    expect(prevented).toBe(true);
    expect(hideWindow).toHaveBeenCalledTimes(1);
    expect(confirmQuit).not.toHaveBeenCalled();
    expect(requestQuit).not.toHaveBeenCalled();
  });

  it("treats missing linux tray capability as unsupported and falls back to quit confirmation", () => {
    const logger = { info: vi.fn() };
    const hideWindow = vi.fn();
    const confirmQuit = vi.fn(() => true);
    const requestQuit = vi.fn();

    // 能力未探测完成（缓存为空）与探测失败都必须走安全默认，绝不能隐藏窗口导致失联。
    for (const closeToTraySupported of [undefined, false] as const) {
      hideWindow.mockClear();
      confirmQuit.mockClear();
      requestQuit.mockClear();
      const prevented = handleDesktopWindowCloseRequest({
        platform: "linux",
        forceQuit: false,
        closeToTray: true,
        closeToTraySupported,
        isLastWindow: true,
        label: "local-linux-no-tray",
        logger,
        confirmQuit,
        requestQuit,
        hideWindow,
      });

      expect(prevented).toBe(true);
      expect(hideWindow).not.toHaveBeenCalled();
      expect(confirmQuit).toHaveBeenCalledTimes(1);
      expect(requestQuit).toHaveBeenCalledTimes(1);
    }
  });

  it("hides windows window to tray on the happy path when tray instance is ready", () => {
    const logger = { info: vi.fn() };
    const hideWindow = vi.fn();
    const confirmQuit = vi.fn(() => true);
    const requestQuit = vi.fn();

    const prevented = handleDesktopWindowCloseRequest({
      platform: "win32",
      forceQuit: false,
      closeToTray: true,
      closeToTraySupported: true,
      isLastWindow: true,
      label: "local-win32-tray",
      logger,
      confirmQuit,
      requestQuit,
      hideWindow,
    });

    expect(prevented).toBe(true);
    expect(hideWindow).toHaveBeenCalledTimes(1);
    expect(confirmQuit).not.toHaveBeenCalled();
    expect(requestQuit).not.toHaveBeenCalled();
  });

  it("does not hide windows window when tray instance failed to create", () => {
    const logger = { info: vi.fn() };
    const hideWindow = vi.fn();
    const confirmQuit = vi.fn(() => true);
    const requestQuit = vi.fn();

    // CR:win32 托盘创建失败(trayReady=false)时隐藏即失联,必须走确认退出。
    const prevented = handleDesktopWindowCloseRequest({
      platform: "win32",
      forceQuit: false,
      closeToTray: true,
      closeToTraySupported: false,
      isLastWindow: true,
      label: "local-win32-no-tray",
      logger,
      confirmQuit,
      requestQuit,
      hideWindow,
    });

    expect(prevented).toBe(true);
    expect(hideWindow).not.toHaveBeenCalled();
    expect(confirmQuit).toHaveBeenCalledTimes(1);
    expect(requestQuit).toHaveBeenCalledTimes(1);
  });

  it("falls back to quit confirmation on linux when close-to-tray setting is off", () => {
    const logger = { info: vi.fn() };
    const hideWindow = vi.fn();
    const confirmQuit = vi.fn(() => false);
    const requestQuit = vi.fn();

    const prevented = handleDesktopWindowCloseRequest({
      platform: "linux",
      forceQuit: false,
      closeToTray: false,
      closeToTraySupported: true,
      isLastWindow: true,
      label: "local-linux-off",
      logger,
      confirmQuit,
      requestQuit,
      hideWindow,
    });

    expect(prevented).toBe(true);
    expect(hideWindow).not.toHaveBeenCalled();
    expect(confirmQuit).toHaveBeenCalledTimes(1);
  });
});
