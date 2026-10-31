import { TID_PROJECT_SECTION } from "@escode/shared";

/**
 * 产品目前没有“新建窗口”菜单；macOS 的 activate 是创建主窗口的真实入口。
 * 测试临时把已有窗口视为不存在，走同一 coordinator/loadWindow 路径创建第二个窗口，
 * 而不是手写 BrowserWindow 配置绕过应用初始化。
 */
export async function openSecondMainWindow(caseMarker: string): Promise<void> {
  const handles = await browser.getWindowHandles();
  await browser.electron.execute(async (electron) => {
    const originalWindows = electron.BrowserWindow.getAllWindows;
    const created = new Promise<void>((resolve) => {
      electron.app.once("browser-window-created", () => {
        electron.BrowserWindow.getAllWindows = originalWindows;
        resolve();
      });
    });
    electron.BrowserWindow.getAllWindows = () => [];
    try {
      electron.app.emit("activate");
      await created;
    } finally {
      electron.BrowserWindow.getAllWindows = originalWindows;
    }
  });

  let handle = "";
  await browser.waitUntil(
    async () => {
      handle =
        (await browser.getWindowHandles()).find((candidate) => !handles.includes(candidate)) ?? "";
      return Boolean(handle);
    },
    {
      timeout: 30_000,
      interval: 250,
      timeoutMsg: `${caseMarker}: 第二个主窗口没有出现`,
    },
  );
  await browser.switchToWindow(handle);
}

export async function waitForSecondWindowAppShell(caseMarker: string): Promise<void> {
  await browser.waitUntil(
    () =>
      browser.execute(
        (testId) => Boolean(document.querySelector(`[data-testid="${testId}"]`)),
        TID_PROJECT_SECTION,
      ),
    {
      timeout: 60_000,
      interval: 500,
      timeoutMsg: `${caseMarker}: 第二个窗口没有恢复主界面`,
    },
  );
}

export async function waitForSecondWindowRemoteTab(
  caseMarker: string,
  workspacePath: string,
): Promise<void> {
  try {
    await browser.waitUntil(
      () =>
        browser.execute(
          (expectedPath) =>
            Boolean(
              (
                window as Window & {
                  __escodeTabStoreE2E?: {
                    getState?: () => {
                      tabs?: Array<{ workspacePath?: string }>;
                    };
                  };
                }
              ).__escodeTabStoreE2E
                ?.getState?.()
                .tabs?.some((tab) => tab.workspacePath === expectedPath),
            ),
          workspacePath,
        ),
      {
        timeout: 30_000,
        interval: 500,
        timeoutMsg: `${caseMarker}: 第二个窗口没有恢复远端 tab`,
      },
    );
  } catch (error) {
    const diagnostics = await browser.execute(() => {
      const tabState = (
        window as Window & {
          __escodeTabStoreE2E?: { getState?: () => unknown };
        }
      ).__escodeTabStoreE2E?.getState?.();
      const sessionState = (
        window as Window & {
          __remoteWorkspaceSessionStoreE2E?: { getState?: () => unknown };
        }
      ).__remoteWorkspaceSessionStoreE2E?.getState?.();
      return {
        href: window.location.href,
        sessionState,
        tabState,
      };
    });
    throw new Error(
      `${error instanceof Error ? error.message : String(error)} ${JSON.stringify(diagnostics)}`,
      { cause: error },
    );
  }
}
