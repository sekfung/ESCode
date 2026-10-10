import { recordArmsCustomEventForE2E } from "@escode/ui";
import { DesktopCommandIds, buildLocalMediaPreviewUrl, type IPlatformService } from "@escode/shared";

import { desktopBrowserPlatformBridge } from "./desktopBrowserPlatformBridge.js";

export function createDesktopPlatform(options: {
  isLocalDevelopmentRuntime: boolean;
}): IPlatformService {
  return {
    canSelectFilePath: true,
    supportsSerialPort: true,
    createLocalMediaPreviewUrl: buildLocalMediaPreviewUrl,
    isLocalDevelopmentRuntime: options.isLocalDevelopmentRuntime,
    selectDirectory: () => window.escode.selectDirectory(),
    selectFile: () => window.escode.selectFile(),
    selectFiles: () => window.escode.selectFiles?.() ?? Promise.resolve([]),
    createTempTextAttachment: (payload) => window.escode.createTempTextAttachment(payload),
    onRemoteConnectionLog: (handler) => window.escode.onRemoteConnectionLog(handler),
    onRemoteSessionClosed: (handler) => window.escode.onRemoteSessionClosed(handler),
    onBotRemoteWorkspaceReconnected: (handler) =>
      window.escode.onBotRemoteWorkspaceReconnected(handler),
    activateOrSetWorkspace: (path) =>
      window.escode.activateOrSetWorkspace?.(path) ?? Promise.resolve({ activated: false }),
    connectRemote: (remoteOptions, requestId, context) =>
      window.escode.connectRemote(remoteOptions, requestId, context),
    cancelPendingRemoteConnection: (requestId) =>
      window.escode.cancelPendingRemoteConnection?.(requestId) ?? Promise.resolve(),
    bindRemoteWorkspaceSessionContext: (context) =>
      window.escode.bindRemoteWorkspaceSessionContext?.(context) ?? Promise.resolve(),
    disposeRemoteSession: (sessionId) => window.escode.disposeRemoteSession(sessionId),
    isDockerAvailable: () => window.escode.isDockerAvailable(),
    listWSLDistros: () => window.escode.listWSLDistros(),
    listDockerContainers: () => window.escode.listDockerContainers(),
    listSSHConfigAliases: () => window.escode.listSSHConfigAliases(),
    loadMcpFromUserDirectory: (payload) => window.escode.loadMcpFromUserDirectory(payload),
    saveMcpToUserDirectory: (payload) => window.escode.saveMcpToUserDirectory(payload),
    migrateLegacyCommonMcp: (payload) => window.escode.migrateLegacyCommonMcp(payload),
    openExternal: (url) => window.escode.openExternal(url),
    openFeedback: () => window.escode.executeDesktopCommand(DesktopCommandIds.OpenFeedback),
    openCommunity: () => window.escode.executeDesktopCommand(DesktopCommandIds.OpenCommunity),
    canOpenCommunity: (locale) => window.escode.canOpenCommunity(locale),
    openInFileManager: (path) => window.escode.openInFileManager(path),
    openExternalFile: (path) => window.escode.openExternalFile(path),
    openCuaPermissionOnboarding: window.escode.openCuaPermissionOnboarding
      ? (permissionOptions) =>
          window.escode.openCuaPermissionOnboarding?.(permissionOptions) ??
          Promise.resolve({ success: false, error: "not_supported" })
      : undefined,
    prepareCuaHelperPermissionDrag: window.escode.prepareCuaHelperPermissionDrag
      ? () =>
          window.escode.prepareCuaHelperPermissionDrag?.() ??
          Promise.resolve({ success: false, error: "not_supported" })
      : undefined,
    startCuaHelperPermissionDrag: window.escode.startCuaHelperPermissionDrag
      ? () => window.escode.startCuaHelperPermissionDrag?.()
      : undefined,
    registerOAuthState: (payload) => window.escode.registerOAuthState(payload),
    onOAuthCallback: (callback) => window.escode.onOAuthCallback(callback),
    onPaymentCallback: (callback) => window.escode.onPaymentCallback(callback),
    onShareImport: (callback) => window.escode.onShareImport?.(callback) ?? (() => {}),
    notifyRendererReady: () => window.escode.notifyRendererReady(),
    reportTelemetryEvent: (payload) => window.escode.reportTelemetryEvent(payload),
    reportArmsCustomEvent: (payload) => {
      recordArmsCustomEventForE2E(payload);
      return window.escode.reportArmsCustomEvent(payload);
    },
    getRendererActionTraceConfig: window.escode.getRendererActionTraceConfig
      ? () => window.escode.getRendererActionTraceConfig!()
      : undefined,
    onRendererActionTraceConfigChanged: window.escode.onRendererActionTraceConfigChanged
      ? (callback) => window.escode.onRendererActionTraceConfigChanged!(callback)
      : undefined,
    reportLocalTtftBatch: (batch) => window.escode.reportLocalTtftBatch(batch),
    reportRendererActionTraceBatch: window.escode.reportRendererActionTraceBatch
      ? (batch) => window.escode.reportRendererActionTraceBatch!(batch)
      : undefined,
    reportRendererHeapSample: window.escode.reportRendererHeapSample
      ? (sample) => window.escode.reportRendererHeapSample!(sample)
      : undefined,
    showTaskNotification: (payload) => window.escode.showTaskNotification(payload),
    syncWindowTabs: (paths) => window.escode.syncWindowTabs(paths),
    syncWindowUnreadCount: (count) => window.escode.syncWindowUnreadCount(count),
    syncActiveTaskSession: (sessionId) => window.escode.syncActiveTaskSession(sessionId),
    syncAppSettings: (patch) => window.escode.syncAppSettings?.(patch),
    setShortcutRecordingActive: (active) => window.escode.setShortcutRecordingActive?.(active),
    onFocusTab: (handler) => window.escode.onFocusTab(handler),
    onNewTab: (handler) => window.escode.onNewTab(handler),
    onCloseActiveContextRequest: (handler) =>
      window.escode.onCloseActiveContextRequest?.(handler) ?? (() => {}),
    onOpenBrowserUrl: (handler) => window.escode.onOpenBrowserUrl?.(handler) ?? (() => {}),
    onBrowserViewScreenshotSurfacePrepare: (handler) =>
      window.escode.onBrowserViewScreenshotSurfacePrepare?.(handler) ?? (() => {}),
    onBrowserViewScreenshotSurfaceRelease: (handler) =>
      window.escode.onBrowserViewScreenshotSurfaceRelease?.(handler) ?? (() => {}),
    browserViewScreenshotSurfaceReady: (payload) =>
      window.escode.browserViewScreenshotSurfaceReady?.(payload),
    ...desktopBrowserPlatformBridge,
    onNewTask: (handler) => window.escode.onNewTask(handler),
    onOpenWorkspace: (handler) => {
      // 开发态或升级后的旧窗口可能仍运行未暴露 onOpenWorkspace 的 preload，
      // renderer 直接调用会在启动时崩溃。这里和 activateOrSetWorkspace 一样做兼容兜底，
      // 缺少该 bridge 时只禁用原生菜单回调，不影响应用继续打开。
      return window.escode.onOpenWorkspace?.(handler) ?? (() => {});
    },
    onOpenWorkspacePath: (handler) => window.escode.onOpenWorkspacePath?.(handler) ?? (() => {}),
    onOpenFeedbackDialog: (handler) => window.escode.onOpenFeedbackDialog?.(handler) ?? (() => {}),
    onOpenTicketsPanel: (handler) => window.escode.onOpenTicketsPanel?.(handler) ?? (() => {}),
    onWindowFullscreenChanged: (handler) => window.escode.onWindowFullscreenChanged(handler),
    getDesktopWindowChromeState: window.escode.getDesktopWindowChromeState
      ? () => window.escode.getDesktopWindowChromeState!()
      : undefined,
    onDesktopWindowChromeStateChanged: window.escode.onDesktopWindowChromeStateChanged
      ? (handler) => window.escode.onDesktopWindowChromeStateChanged!(handler)
      : undefined,
    getWindowControlsOverlayMetrics: () => window.escode.getWindowControlsOverlayMetrics?.() ?? null,
    onWindowControlsOverlayChanged: (handler) =>
      window.escode.onWindowControlsOverlayChanged?.(handler) ?? (() => {}),
    getDesktopZoomLevel: () =>
      window.escode.getDesktopZoomLevel?.() ?? Promise.resolve({ zoomLevel: 0 }),
    onDesktopZoomLevelChanged: (handler) =>
      window.escode.onDesktopZoomLevelChanged?.(handler) ?? (() => {}),
    onTaskNotificationClick: (handler) => window.escode.onTaskNotificationClick(handler),
    exportLogs: () => window.escode.exportLogs(),
    captureWindowScreenshot: () =>
      window.escode.captureWindowScreenshot?.() ?? Promise.resolve(null),
    onUpdateReady: (callback) => window.escode.onUpdateReady(callback),
    onUpdateCheckResult: (callback) => window.escode.onUpdateCheckResult(callback),
    onUpdateStateChanged: (callback) => window.escode.onUpdateStateChanged?.(callback) ?? (() => {}),
    getUpdateState: () =>
      window.escode.getUpdateState?.() ?? Promise.resolve({ kind: "idle", enabled: true }),
    downloadUpdate: () => window.escode.downloadUpdate?.() ?? Promise.resolve(),
    cancelUpdateDownload: () => window.escode.cancelUpdateDownload?.() ?? Promise.resolve(),
    openUpdateStatusWindow: () => window.escode.openUpdateStatusWindow?.() ?? Promise.resolve(),
    getAutoUpdatePreferences: () =>
      window.escode.getAutoUpdatePreferences?.() ??
      Promise.resolve({ autoDownloadAndInstallUpdates: false }),
    setAutoDownloadAndInstallUpdates: (enabled) =>
      window.escode.setAutoDownloadAndInstallUpdates?.(enabled) ?? Promise.resolve(),
    getDesktopSessionActivity: () =>
      window.escode.getDesktopSessionActivity?.() ??
      Promise.resolve({ runningAgentSessionCount: 0 }),
    getESCodeStdioTapDevState: () =>
      window.escode.getESCodeStdioTapDevState?.() ??
      Promise.resolve({ enabled: false, visible: false, logDir: "", statePath: "" }),
    onSettingsChanged: (callback) => window.escode.onSettingsChanged?.(callback) ?? (() => {}),
    onApplicationLocaleChanged: (callback) =>
      window.escode.onApplicationLocaleChanged?.(callback) ?? (() => {}),
    onPostUpdateReleaseNotes: (callback) => window.escode.onPostUpdateReleaseNotes(callback),
    acknowledgePostUpdateReleaseNotes: (version) =>
      window.escode.acknowledgePostUpdateReleaseNotes(version),
    skipUpdateVersion: (version) => window.escode.skipUpdateVersion?.(version) ?? Promise.resolve(),
    quitAndInstallUpdate: () => window.escode.quitAndInstallUpdate(),
    getInstalledEditors: () => window.escode.getInstalledEditors(),
    getApplicationIcon: (bundleId) =>
      window.escode.getApplicationIcon?.(bundleId) ?? Promise.resolve(null),
    openInEditor: (editorId, path, editorOptions) =>
      window.escode.openInEditor(editorId, path, editorOptions),
    executeDesktopCommand: (command) => window.escode.executeDesktopCommand(command),
    setApplicationLocale: (locale) => window.escode.setApplicationLocale(locale),
    getSystemLocale: () =>
      window.escode.getSystemLocale?.() ??
      Promise.resolve(navigator.language.toLowerCase().startsWith("zh") ? "zh-CN" : "en-US"),
    setTitleBarTheme: (theme) => window.escode.setTitleBarTheme(theme),
    getDeviceId: () =>
      (window as Window & { __ESCODE_DEVICE_ID__?: string }).__ESCODE_DEVICE_ID__ ?? "",
  };
}
