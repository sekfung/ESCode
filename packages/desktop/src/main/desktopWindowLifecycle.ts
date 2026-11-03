import { disposeRustHostWindow } from "./desktopRustHostWindow.js";
import { attachRustWindowHostForDomReady } from "./desktopWindowRustHost.js";
import { app, BrowserWindow, Menu } from "electron";
import { PlatformChannels, type Locale } from "@escode/shared";
import { createBrowserWindow } from "./desktopWindowChrome.js";
import type { WindowBootstrapOptions } from "./desktopHostProcess.js";
import type { StartupWorkspaceWarmupTarget } from "./startupWorkspace.js";
import { handleDarwinWindowCloseRequest } from "./desktopDarwinCloseBehavior.js";
import {
  parseWindowUnreadCount,
  sumWindowUnreadCounts,
  syncAppUnreadBadge,
} from "./unreadBadge.js";
import { attachDesktopWindowSizePersistence, type DesktopWindowSize } from "./desktopWindowSize.js";
import {
  registerMainApplicationWindow,
  unregisterMainApplicationWindow,
} from "./resourceManagerWindow.js";

export function createWindow(options: {
  iconPath: string;
  preloadPath: string;
  logger: { info: (...args: unknown[]) => void; warn: (...args: unknown[]) => void };
  forceQuitRef: { current: boolean };
  handleBeforeClose?: (win: BrowserWindow, label: string) => boolean;
  /** S6：窗口经 attachment 接入 App 级 Rust Host，不再 spawn Node utility process。 */
  rustHostManager: import("./rustHostManager.js").RustHostManager;
  syncAutoUpdaterStateToWindow: (win: BrowserWindow) => void;
  syncReadyUpdateToWindow: (win: BrowserWindow) => void;
  syncPostUpdateReleaseNotesToWindow: (win: BrowserWindow) => void;
  disposeRemoteWorkspaceSessionsForWindow: (windowId: number, reason: string) => void;
  reattachRemoteWorkspaceSessionsForWindow: (win: BrowserWindow, reason: string) => void;
  bootstrap?: WindowBootstrapOptions;
  agentWarmupTargets?: readonly StartupWorkspaceWarmupTarget[];
  deviceMid: string;
  initialDesktopZoomLevel?: number;
  initialWindowSize?: DesktopWindowSize;
  currentApplicationLocale?: () => Locale;
  persistWindowSize?: (state: DesktopWindowSize) => Promise<void>;
  /** Main 模块初始化期已开始的异步环境采集；通常在 renderer dom-ready 前完成。 */
  runtimeProcessEnvPatchPromise?: Promise<Record<string, string>>;
  /** 不执行 shell 即可计算的完整降级 patch；预热失败/超时时仍要注入 Local Host。 */
  runtimeProcessEnvFallbackPatch: Record<string, string>;
  /** 仅供启动门禁和测试注入；超过该时间必须 fail-open 创建 Local Host。 */
  runtimeProcessEnvWaitTimeoutMs?: number;
  resolveBrowserViewOwner?: Parameters<typeof createBrowserWindow>[0]["resolveBrowserViewOwner"];
}) {
  const win = createBrowserWindow({
    iconPath: options.iconPath,
    preloadPath: options.preloadPath,
    bootstrap: {
      restoreSession: options.bootstrap?.restoreSession ?? true,
      supportsSettings: options.bootstrap?.supportsSettings ?? true,
      initialWorkspacePath: options.bootstrap?.initialWorkspacePath,
      initialWorkspacePurpose: options.bootstrap?.initialWorkspacePurpose,
      unavailableWorkspacePath: options.bootstrap?.unavailableWorkspacePath,
    },
    logger: options.logger,
    deviceMid: options.deviceMid,
    initialDesktopZoomLevel: options.initialDesktopZoomLevel,
    initialWindowSize: options.initialWindowSize,
    currentApplicationLocale: options.currentApplicationLocale,
    resolveBrowserViewOwner: options.resolveBrowserViewOwner,
  });
  const label = `local-${win.webContents.id}`;

  if (options.persistWindowSize) {
    attachDesktopWindowSizePersistence(win, options.persistWindowSize, (error) => {
      options.logger.warn("[desktop-window] failed to persist main window size", error);
    });
  }

  if (process.platform === "darwin") {
    win.on("close", (event) => {
      if (
        handleDarwinWindowCloseRequest({
          win,
          forceQuit: options.forceQuitRef.current,
          label,
          logger: options.logger,
        })
      ) {
        event.preventDefault();
      }
    });
  } else if (options.handleBeforeClose) {
    win.on("close", (event) => {
      if (options.handleBeforeClose?.(win, label)) {
        event.preventDefault();
      }
    });
  }

  const wcId = win.webContents.id;
  // 资源管理器据此把主窗口 renderer 与辅助窗口区分开。
  registerMainApplicationWindow(wcId);
  let domReadyGeneration = 0;
  win.webContents.on("dom-ready", async () => {
    const currentDomReadyGeneration = ++domReadyGeneration;
    options.logger.info(`[createWindow] dom-ready fired (${label})`);

    if (process.platform === "win32" && !win.isDestroyed()) {
      win.show();
      win.focus();
    }

    // S6：窗口只是一个 Rust Host attachment；reload/close 都不停 Host，也不再有 Node Host 回退。
    disposeRustHostWindow(wcId);
    try {
      const attachment = await attachRustWindowHostForDomReady({
        win,
        label,
        manager: options.rustHostManager,
        logger: options.logger,
        ...(options.agentWarmupTargets ? { agentWarmupTargets: options.agentWarmupTargets } : {}),
        ...(options.runtimeProcessEnvPatchPromise
          ? { runtimeProcessEnvPatchPromise: options.runtimeProcessEnvPatchPromise }
          : {}),
        runtimeProcessEnvFallbackPatch: options.runtimeProcessEnvFallbackPatch,
        ...(options.runtimeProcessEnvWaitTimeoutMs
          ? { runtimeProcessEnvWaitTimeoutMs: options.runtimeProcessEnvWaitTimeoutMs }
          : {}),
        onLost: () => {
          options.logger.warn(`[createWindow] Rust Host attachment lost (${label})`);
        },
      });
      if (currentDomReadyGeneration !== domReadyGeneration || win.isDestroyed()) {
        attachment.dispose();
        return;
      }
      options.syncAutoUpdaterStateToWindow(win);
      options.syncReadyUpdateToWindow(win);
      options.syncPostUpdateReleaseNotesToWindow(win);
      options.reattachRemoteWorkspaceSessionsForWindow(win, `${label}:renderer-ready`);
    } catch (error) {
      // 启动失败显式报错，不回退 Node Host（同 M2 语义）。
      options.logger.warn(`[createWindow] Rust Host attachment failed (${label}):`, error);
    }
  });

  win.on("closed", () => {
    unregisterMainApplicationWindow(wcId);
    disposeRustHostWindow(wcId);
    options.logger.info(`[createWindow] window closed, detaching Rust Host (${label})`);
    void options.disposeRemoteWorkspaceSessionsForWindow(wcId, `${label}:window-closed`);
  });

  return win;
}

export function showCurrentWindowFromDock(primaryWindowCoordinator: {
  ensurePrimaryWindow(reason: string): Promise<void>;
}) {
  if (process.platform === "darwin") {
    app.show();
  }

  void primaryWindowCoordinator.ensurePrimaryWindow("dock-show-current-window");
}

export function focusWorkspaceInExistingWindow(
  path: string,
  windowWorkspaceMap: Map<number, Set<string>>,
  options?: { skipWindowId?: number },
): { activated: boolean; winId?: number } {
  for (const [winId, pathSet] of windowWorkspaceMap) {
    if (options?.skipWindowId === winId) {
      continue;
    }
    if (!pathSet.has(path)) {
      continue;
    }

    const existingWin = BrowserWindow.fromId(winId);
    if (existingWin && !existingWin.isDestroyed()) {
      if (existingWin.isMinimized()) {
        existingWin.restore();
      }
      existingWin.focus();
      existingWin.webContents.send(PlatformChannels.FocusTab, path);
      return { activated: true, winId };
    }

    windowWorkspaceMap.delete(winId);
  }

  return { activated: false };
}

export function syncApplicationUnreadBadge(windowUnreadCountMap: Map<number, number>) {
  syncAppUnreadBadge({
    platform: process.platform,
    totalUnreadCount: sumWindowUnreadCounts(windowUnreadCountMap),
    setBadgeCount: (count) => {
      app.setBadgeCount(count);
    },
  });
}

export function handleWindowUnreadCountSync(
  win: BrowserWindow | null,
  payload: unknown,
  windowUnreadCountMap: Map<number, number>,
  logger: { warn: (...args: unknown[]) => void },
) {
  const unreadCount = parseWindowUnreadCount(payload);
  if (unreadCount == null) {
    logger.warn("[sync-window-unread-count] invalid payload:", payload);
    return false;
  }

  if (!win) {
    return false;
  }

  if (unreadCount === 0) {
    windowUnreadCountMap.delete(win.id);
  } else {
    windowUnreadCountMap.set(win.id, unreadCount);
  }
  syncApplicationUnreadBadge(windowUnreadCountMap);
  return true;
}

export function configureDockMenu(getLabel: () => string, onShowCurrentWindow: () => void) {
  if (process.platform !== "darwin" || app.dock == null) {
    return;
  }

  const dockMenu = Menu.buildFromTemplate([
    {
      label: getLabel(),
      click: onShowCurrentWindow,
    },
  ]);

  app.dock.setMenu(dockMenu);
}

export function handleDesktopWindowCloseRequest(options: {
  platform: NodeJS.Platform;
  forceQuit: boolean;
  explicitQuitRequested?: boolean;
  closeToTray?: boolean;
  /**
   * Linux 关闭驻留的托盘能力（spec：docs/desktop/linux-close-to-tray.md）。
   * GNOME 等无 StatusNotifierWatcher 的桌面托盘不渲染且不报错，必须在隐藏前显式确认能力；
   * 空值按"不可用"安全默认，走确认退出路径，避免窗口隐藏后失联。
   */
  closeToTraySupported?: boolean;
  isLastWindow: boolean;
  label: string;
  logger: { info: (...args: unknown[]) => void };
  shouldConfirmQuit?: boolean;
  confirmQuit: () => boolean;
  requestQuit: () => void;
  hideWindow?: () => void;
}) {
  // CR 修复:win32 不能短路丢弃 closeToTraySupported——它承载 resolveCloseToTraySupported
  // 的合并裁决(Windows=托盘实例创建成功)。托盘创建失败时隐藏窗口即失联,与 spec
  // "关窗分支与设置页能力查询都读同一合并结果"保持一致。
  const canHideToTray =
    (options.platform === "win32" || options.platform === "linux") &&
    options.closeToTraySupported === true;
  if (
    canHideToTray &&
    options.closeToTray &&
    !options.forceQuit &&
    !options.explicitQuitRequested
  ) {
    options.logger.info(`[createWindow] window close hidden to tray (${options.label})`);
    options.hideWindow?.();
    return true;
  }

  if (options.platform === "darwin" || options.forceQuit || !options.isLastWindow) {
    return false;
  }

  if (options.shouldConfirmQuit === false) {
    options.logger.info(
      `[createWindow] last window close skipped confirmation, quitting app (${options.label})`,
    );
    options.requestQuit();
    return true;
  }

  if (!options.confirmQuit()) {
    options.logger.info(`[createWindow] last window close canceled by user (${options.label})`);
    return true;
  }

  options.logger.info(
    `[createWindow] last window close confirmed, quitting app (${options.label})`,
  );
  options.requestQuit();
  return true;
}
