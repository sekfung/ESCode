import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { beforeEach, describe, expect, it, vi } from "vitest";

const mockSetUser = vi.fn();
const mockModelSelectionRead = { failed: false };
vi.mock("@/hooks/useModelSelectionView.js", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useModelSelectionServiceView: () => ({
    state: mockModelSelectionRead.failed
      ? { status: "error", error: new Error("fixture provider read failed") }
      : { status: "loading" },
    reload: vi.fn(),
  }),
}));
const mockSetOAuthError = vi.fn();
const mockAddTab = vi.fn();
const mockActivateTabByPath = vi.fn(() => false);
const mockStartDraft = vi.fn();
const mockShouldBlockRootRender = vi.fn(() => false);
const mockUseGlobalTaskList = vi.fn(() => ({
  items: [],
  total: 0,
  hasMore: false,
  loading: false,
  syncingRemoteWorkspaces: false,
  refresh: vi.fn(),
}));

const tabStoreState = {
  tabs: [] as Array<{
    id: string;
    type: "workspace" | "settings";
    workspacePath?: string;
    label?: string;
  }>,
  activeTabId: null as string | null,
  activeWorkspacePath: null as string | null,
  addTab: mockAddTab,
  activateTabByPath: mockActivateTabByPath,
};

const tabStoreApiState = {
  activeWorkspacePath: null as string | null,
  activateTabByPath: mockActivateTabByPath,
};

vi.mock("@/App.js", () => ({
  App: () => createElement("div", { "data-testid": "workspace-app" }),
}));

vi.mock("@/ConfirmDialog.js", () => ({
  ConfirmDialogHost: () => createElement("div", { "data-testid": "confirm-dialog-host" }),
}));

vi.mock("@/hooks/usePlatform.js", () => ({
  PlatformProvider: ({ children }: { children: unknown }) => children,
  useOptionalPlatform: () => ({}),
  usePlatform: () => ({
    cancelPendingRemoteConnection: vi.fn(async () => {}),
  }),
}));

vi.mock("@/hooks/useServices.js", () => ({
  ServiceProvider: ({ children }: { children: unknown }) => children,
  useServices: () => ({
    systemService: {
      probeIntranet: vi.fn(async () => ({
        isIntranet: false,
        targets: [],
      })),
    },
  }),
  useOptionalServices: () => null,
}));

vi.mock("@/settings/CodingPlanUpgradeDialogProvider.js", () => ({
  CodingPlanUpgradeDialogProvider: ({ children }: { children: unknown }) => children,
  useCodingPlanUpgradeDialog: () => ({
    openCodingPlanUpgrade: vi.fn(),
  }),
}));

vi.mock("@/hooks/useWorkspaceServices.js", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/hooks/useWorkspaceServices.js")>();

  return {
    ...actual,
    // Bugfix: Root 现在会触发 useBaseWorkspaceServices；这里改为部分 mock，
    // 保留模块真实导出，避免新增 hook 导出时测试因 mock 漏项直接崩溃。
    // 此 hook 的首参是 workspacePath。旧 mock 原样返回首参，只因早期 return 未读取服务才未暴露。
    // 数据库 loading 提前订阅工作区服务后，测试也必须返回真实契约形状。
    useWorkspaceServices: () => ({
      escodeAgentService: {
        prepareStorage: vi.fn(async () => {}),
        getStorageStartupState: vi.fn(async () => null),
        onDynamicStorageStartupState: vi.fn(() => () => ({ dispose: vi.fn() })),
      },
    }),
  };
});

vi.mock("@/hooks/useTabPersistence.js", () => ({
  useTabPersistence: () => ({
    isRestoring: false,
    hasCompletedInitialRestore: true,
    hasCompletedFullRestore: true,
  }),
}));

vi.mock("@/hooks/useGlobalTaskList.js", () => ({
  useGlobalTaskList: mockUseGlobalTaskList,
}));

vi.mock("@/hooks/useTokenRefresh.js", () => ({
  useTokenRefresh: () => ({
    tryRefresh: vi.fn(),
    clearCredentials: vi.fn(),
  }),
}));

vi.mock("@/i18n/IntlProvider.js", () => ({
  useESCodeIntl: () => ({
    intl: {
      formatMessage: vi.fn(() => "loading"),
    },
  }),
}));

vi.mock("@/logger.js", () => ({
  logger: {
    info: vi.fn(),
    warn: vi.fn(),
    error: vi.fn(),
  },
}));

vi.mock("@/SettingsPage.js", () => ({
  SettingsPage: () => createElement("div", { "data-testid": "settings-page" }),
}));

vi.mock("@/WelcomeScreen.js", () => ({
  WelcomeScreen: () => createElement("div", { "data-testid": "welcome-screen" }),
}));

vi.mock("@/lib/fileDisplay.js", () => ({
  setDefaultFileDisplayBasePath: vi.fn(),
}));

vi.mock("@/lib/rootStartupGate.js", () => ({
  isProviderStartupSyncPending: () => false,
  shouldEnableProviderAvailabilityLoginEntryGuard: () => true,
  shouldResolveProviderStartupState: ({
    providerStartupSyncPending,
    providerAvailabilityStartupCheckCompleted,
  }: {
    providerStartupSyncPending: boolean;
    providerAvailabilityStartupCheckCompleted: boolean;
  }) => providerStartupSyncPending || !providerAvailabilityStartupCheckCompleted,
  shouldBlockRootRender: () => mockShouldBlockRootRender(),
  shouldShowRootStartupLoading: ({
    isDesktop,
    welcomeScreenOpen,
  }: {
    isDesktop?: boolean;
    welcomeScreenOpen: boolean;
  }) => Boolean(isDesktop) && !welcomeScreenOpen && mockShouldBlockRootRender(),
}));

vi.mock("@/store/StoreProvider.js", () => ({
  StoreProvider: ({ children }: { children: unknown }) => children,
  useESCodeStore: (selector: (state: unknown) => unknown) =>
    selector({
      user: null,
      setUser: mockSetUser,
      setOAuthError: mockSetOAuthError,
    }),
}));

vi.mock("@/store/escodeSessionStore.js", () => ({
  useESCodeSessionStore: Object.assign(
    (selector: (state: unknown) => unknown) =>
      selector({
        workspaces: {},
      }),
    {
      getState: () => ({
        startDraft: mockStartDraft,
        workspaces: {},
      }),
    },
  ),
}));

vi.mock("@/store/TabStoreProvider.js", () => ({
  TabStoreProvider: ({ children }: { children: unknown }) => children,
  useTabStore: (selector: (state: unknown) => unknown) => selector(tabStoreState),
  useTabStoreApi: () => ({
    getState: () => tabStoreApiState,
  }),
}));

vi.mock("@/store/tabStore.js", () => ({
  isSettingsTab: (tab: { type?: string } | null | undefined) => tab?.type === "settings",
  isWorkspaceTab: (tab: { type?: string } | null | undefined) => tab?.type === "workspace",
}));

vi.mock("@/store/remoteWorkspaceSessionStore.js", () => ({
  bindRemoteWorkspacePath: vi.fn(),
  getRemoteWorkspaceSession: vi.fn(() => null),
  unbindRemoteWorkspacePath: vi.fn(),
  unregisterRemoteWorkspaceSession: vi.fn(),
  useRemoteWorkspaceSessionStore: (selector: (state: unknown) => unknown) =>
    selector({
      sessionsById: {},
      sessionIdByWorkspacePath: {},
      sessionIdByWorkspaceIdentity: {},
    }),
}));

vi.mock("@/UpdateStatusButton.js", () => ({
  UpdateStatusButton: () => createElement("div", { "data-testid": "update-ready-button" }),
}));

async function renderRoot(options?: {
  isDesktop?: boolean;
  initialWorkspaceAbsPath?: string;
  platformOverrides?: Record<string, unknown>;
}) {
  const { Root } = await import("@/Root.js");
  const html = renderToStaticMarkup(
    createElement(Root, {
      services: {
        broadcastService: {},
        settingService: {
          get: vi.fn(),
          update: vi.fn(),
        },
        oauthService: {
          restoreCachedSession: vi.fn(),
          restoreSession: vi.fn(),
          handleCallback: vi.fn(),
          logout: vi.fn(),
        },
      },
      platform: {
        selectDirectory: vi.fn(async () => null),
        activateOrSetWorkspace: vi.fn(async () => ({ activated: false })),
        connectRemote: vi.fn(async () => ({ success: true })),
        disposeRemoteSession: vi.fn(async () => {}),
        isDockerAvailable: vi.fn(async () => false),
        listWSLDistros: vi.fn(async () => []),
        listDockerContainers: vi.fn(async () => []),
        listSSHConfigAliases: vi.fn(async () => []),
        openExternal: vi.fn(),
        openCommunity: vi.fn(async () => {}),
        canOpenCommunity: vi.fn(async () => false),
        openInFileManager: vi.fn(async () => ({ success: true })),
        registerOAuthState: vi.fn(),
        onOAuthCallback: vi.fn(() => () => {}),
        notifyRendererReady: vi.fn(),
        showTaskNotification: vi.fn(),
        syncWindowTabs: vi.fn(),
        syncWindowUnreadCount: vi.fn(),
        onFocusTab: vi.fn(() => () => {}),
        onNewTab: vi.fn(() => () => {}),
        onNewTask: vi.fn(() => () => {}),
        onOpenWorkspace: vi.fn(() => () => {}),
        onWindowFullscreenChanged: vi.fn(() => () => {}),
        onTaskNotificationClick: vi.fn(() => () => {}),
        exportLogs: vi.fn(async () => ({
          success: false,
          error: "unsupported",
        })),
        onUpdateReady: vi.fn(() => () => {}),
        onUpdateStateChanged: vi.fn(() => () => {}),
        getUpdateState: vi.fn(async () => ({ kind: "idle", enabled: true })),
        onUpdateCheckResult: vi.fn(() => () => {}),
        onPostUpdateReleaseNotes: vi.fn(() => () => {}),
        quitAndInstallUpdate: vi.fn(async () => {}),
        getInstalledEditors: vi.fn(async () => []),
        openInEditor: vi.fn(),
        executeDesktopCommand: vi.fn(async () => {}),
        setApplicationLocale: vi.fn(async () => {}),
        setTitleBarTheme: vi.fn(async () => {}),
        ...options?.platformOverrides,
      },
      isDesktop: options?.isDesktop,
      initialWorkspaceAbsPath: options?.initialWorkspaceAbsPath,
    }),
  );

  return html;
}

describe("Root update ready overlay", () => {
  it("模型读取失败时仍能渲染重试入口，不因图标缺失崩溃", async () => {
    mockModelSelectionRead.failed = true;
    try {
      const html = await renderRoot();
      expect(html).toContain("lucide-refresh-cw");
    } finally {
      mockModelSelectionRead.failed = false;
    }
  }, 15_000);
  beforeEach(() => {
    tabStoreState.tabs = [];
    tabStoreState.activeTabId = null;
    tabStoreState.activeWorkspacePath = null;
    tabStoreApiState.activeWorkspacePath = null;
    mockSetUser.mockClear();
    mockSetOAuthError.mockClear();
    mockAddTab.mockClear();
    mockActivateTabByPath.mockClear();
    mockStartDraft.mockClear();
    mockShouldBlockRootRender.mockReset();
    mockShouldBlockRootRender.mockReturnValue(false);
    mockUseGlobalTaskList.mockClear();
  });

  it("无 workspace 首屏不会挂载更新按钮", async () => {
    // Bugfix: Root 现在会串起更多顶层模块，测试环境首轮 import + SSR 偶发超过默认 5s。
    // 这里放宽到局部超时，避免把“模块图变重”误判成“更新按钮逻辑回归”。
    const html = await renderRoot();

    expect(html).not.toContain('data-testid="project-selector"');
    expect(html).not.toContain('data-testid="update-ready-button"');
  }, 15_000);

  it("独立设置页不会挂载更新按钮", async () => {
    tabStoreState.tabs = [{ id: "settings", type: "settings" }];
    tabStoreState.activeTabId = "settings";

    const html = await renderRoot();

    expect(html).toContain('data-testid="settings-page"');
    expect(html).not.toContain('data-testid="update-ready-button"');
  }, 15_000);
});

describe("Root startup render block desktop gate", () => {
  beforeEach(() => {
    tabStoreState.tabs = [];
    tabStoreState.activeTabId = null;
    tabStoreState.activeWorkspacePath = null;
    tabStoreApiState.activeWorkspacePath = null;
    mockShouldBlockRootRender.mockReset();
    mockShouldBlockRootRender.mockReturnValue(false);
  });

  it("桌面启动阻塞态会挂载 RootStartupLoading", async () => {
    mockShouldBlockRootRender.mockReturnValue(true);

    const html = await renderRoot({ isDesktop: true });

    expect(html).toContain('data-testid="root-startup-loading"');
  }, 15_000);

  it("Web 端不使用启动阻塞 gate", async () => {
    mockShouldBlockRootRender.mockImplementation(() => {
      throw new Error("web should not call shouldBlockRootRender");
    });

    await expect(renderRoot({ isDesktop: false })).resolves.not.toContain(
      'data-testid="root-startup-loading"',
    );
  }, 15_000);
});
