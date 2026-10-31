import {
  TID_DIRECTORY_BROWSER_PATH,
  TID_PROJECT_ADD,
  TID_PROJECT_ADD_REMOTE_CONNECTION,
  TID_PROJECT_SECTION,
  TID_SSH_SUCCESS,
  TID_WORKSPACE_RECONNECT,
  testId,
} from "@escode/shared";
import { clickTestIdByWebDriver, hoverTestIdByWebDriver } from "./desktop-app.js";
import { clickLastEnabledSSHDialogButton } from "./ssh-remote-p0.js";

export async function saveCredential(
  caseMarker: string,
  key: string,
  value: string,
): Promise<void> {
  const result = await browser.executeAsync(
    (credentialKey, credentialValue, done) => {
      const services = (
        window as Window & {
          __remoteWorkspaceSessionStoreE2E?: {
            getState?: () => {
              baseServices?: {
                credentialService?: { save: (key: string, value: string) => Promise<void> };
              } | null;
            };
          };
        }
      ).__remoteWorkspaceSessionStoreE2E?.getState?.().baseServices;
      if (!services?.credentialService) {
        done({ ok: false, reason: "credential service unavailable" });
        return;
      }
      void services.credentialService.save(credentialKey, credentialValue).then(
        () => done({ ok: true }),
        (error: unknown) => done({ ok: false, reason: String(error) }),
      );
    },
    key,
    value,
  );
  if (!(result as { ok?: boolean } | null)?.ok) {
    throw new Error(`${caseMarker}: 写入 E2E 凭据失败 ${JSON.stringify(result)}`);
  }
}

/**
 * 已保存设备目录页同时渲染设备重命名输入框，不能像首次直连那样取第一个 text input。
 * 这里只按 DirectoryBrowser 的显式 testid 输入父目录，再逐级进入目标目录。
 */
export async function selectRemoteDirectoryByPath(
  caseMarker: string,
  workspacePath: string,
): Promise<void> {
  // 修复原因：侧栏连接成功后向导可能同时保留上一阶段的隐藏节点；只按 testid 取第一个
  // 会命中不可交互的隐藏输入（Chromium 报 invalid element state）。这里统一操作当前
  // 可见的 DirectoryBrowser，并等远端 home 加载完成（input.value 非空）再写入路径。
  await browser.waitUntil(
    () =>
      browser.execute(
        (successTestId, pathTestId) => {
          const success = Array.from(
            document.querySelectorAll<HTMLElement>(`[data-testid="${successTestId}"]`),
          ).find((element) => element.getClientRects().length > 0);
          const input = success?.querySelector<HTMLInputElement>(`[data-testid="${pathTestId}"]`);
          return (
            Boolean(input?.value) &&
            Boolean(input?.getClientRects().length) &&
            input?.disabled === false
          );
        },
        TID_SSH_SUCCESS,
        TID_DIRECTORY_BROWSER_PATH,
      ),
    {
      timeout: 30_000,
      interval: 100,
      timeoutMsg: `${caseMarker}: 远端目录选择器初始目录未就绪`,
    },
  );

  const normalizedPath = workspacePath.replace(/\/+$/, "") || "/";
  const lastSlash = normalizedPath.lastIndexOf("/");
  const parentPath = lastSlash === 0 ? "/" : normalizedPath.slice(0, lastSlash);
  const directoryName = normalizedPath.slice(lastSlash + 1);
  if (!directoryName) {
    throw new Error(`${caseMarker}: 暂不支持选择根目录`);
  }

  const wroteParentPath = await browser.execute(
    (successTestId, pathTestId, value) => {
      const success = Array.from(
        document.querySelectorAll<HTMLElement>(`[data-testid="${successTestId}"]`),
      ).find((element) => element.getClientRects().length > 0);
      const input = success?.querySelector<HTMLInputElement>(`[data-testid="${pathTestId}"]`);
      if (!input || input.disabled) return false;
      input.focus();
      const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set;
      setter?.call(input, value);
      input.dispatchEvent(new Event("input", { bubbles: true }));
      input.dispatchEvent(new Event("change", { bubbles: true }));
      return true;
    },
    TID_SSH_SUCCESS,
    TID_DIRECTORY_BROWSER_PATH,
    parentPath,
  );
  if (!wroteParentPath) {
    throw new Error(`${caseMarker}: 远端目录选择器路径输入框不可写入`);
  }
  await browser.keys("Enter");
  try {
    await browser.waitUntil(
      async () =>
        browser.execute(
          (successTestId, expectedName) => {
            const success = Array.from(
              document.querySelectorAll<HTMLElement>(`[data-testid="${successTestId}"]`),
            ).find((element) => element.getClientRects().length > 0);
            const entry = Array.from(
              success?.querySelectorAll<HTMLButtonElement>("button") ?? [],
            ).find((button) => button.innerText.trim() === expectedName);
            if (!entry) return false;
            entry.click();
            return true;
          },
          TID_SSH_SUCCESS,
          directoryName,
        ),
      {
        timeout: 30_000,
        interval: 100,
        timeoutMsg: `${caseMarker}: 父目录没有显示目标目录 ${directoryName}`,
      },
    );
  } catch (error) {
    const diagnostics = await browser.execute(
      (successTestId, pathTestId) => {
        const success = Array.from(
          document.querySelectorAll<HTMLElement>(`[data-testid="${successTestId}"]`),
        ).find((element) => element.getClientRects().length > 0);
        const pathInput = success?.querySelector<HTMLInputElement>(`[data-testid="${pathTestId}"]`);
        return {
          bodyText: (document.body.innerText ?? "").slice(0, 3_000),
          buttons: Array.from(success?.querySelectorAll<HTMLButtonElement>("button") ?? []).map(
            (button) => button.innerText.trim(),
          ),
          pathInputValue: pathInput?.value ?? null,
        };
      },
      TID_SSH_SUCCESS,
      TID_DIRECTORY_BROWSER_PATH,
    );
    throw new Error(
      `${caseMarker}: 父目录没有显示目标目录 ${directoryName}: ${JSON.stringify(diagnostics)}`,
      { cause: error },
    );
  }
  await browser.waitUntil(
    () =>
      browser.execute(
        (successTestId, pathTestId, expectedPath) => {
          const success = Array.from(
            document.querySelectorAll<HTMLElement>(`[data-testid="${successTestId}"]`),
          ).find((element) => element.getClientRects().length > 0);
          const input = success?.querySelector<HTMLInputElement>(`[data-testid="${pathTestId}"]`);
          return input?.value === expectedPath;
        },
        TID_SSH_SUCCESS,
        TID_DIRECTORY_BROWSER_PATH,
        normalizedPath,
      ),
    {
      timeout: 30_000,
      interval: 100,
      timeoutMsg: `${caseMarker}: 目录浏览器没有进入目标路径 ${normalizedPath}`,
    },
  );
  await clickLastEnabledSSHDialogButton(caseMarker);
}

export async function openRemoteProfile(
  caseMarker: string,
  profileId: string,
  sourceServerId: string,
): Promise<void> {
  await hoverTestIdByWebDriver(TID_PROJECT_SECTION, {
    timeout: 30_000,
    timeoutMsg: `${caseMarker}: 侧栏没有显示项目分区`,
  });
  await clickTestIdByWebDriver(TID_PROJECT_ADD, {
    timeout: 30_000,
    timeoutMsg: `${caseMarker}: 项目分区没有显示添加项目入口`,
  });
  await clickTestIdByWebDriver(TID_PROJECT_ADD_REMOTE_CONNECTION, {
    timeout: 30_000,
    timeoutMsg: `${caseMarker}: 添加项目菜单没有显示远程连接入口`,
  });
  try {
    const row = $(`[data-remote-source-server-id="${sourceServerId}"] button`);
    await row.waitForClickable({
      timeout: 30_000,
      timeoutMsg: `${caseMarker}: 远程设备页没有显示 profile ${profileId} 的连接入口`,
    });
    await row.click();
  } catch (error) {
    const diagnostics = await browser.execute(() => ({
      bodyText: (document.body.innerText ?? "").slice(0, 2_000),
      visibleTestIds: Array.from(document.querySelectorAll("[data-testid]"))
        .filter((element) => (element as HTMLElement).getClientRects().length > 0)
        .map((element) => element.getAttribute("data-testid")),
    }));
    throw new Error(
      `${error instanceof Error ? error.message : String(error)} ${JSON.stringify(diagnostics)}`,
      { cause: error },
    );
  }
}

export async function clickRemoteWorkspaceReconnect(
  caseMarker: string,
  workspacePath: string,
): Promise<void> {
  const selector = `[data-testid="${testId(TID_WORKSPACE_RECONNECT, workspacePath)}"]`;
  await $(selector).waitForClickable({
    timeout: 30_000,
    timeoutMsg: `${caseMarker}: 侧栏没有显示 ${workspacePath} 的重连入口`,
  });
  await $(selector).click();
}

export async function waitForProfileState(
  caseMarker: string,
  profileId: string,
  expected: string,
  timeoutMs = 60_000,
): Promise<void> {
  let latest: string | null = null;
  await browser.waitUntil(
    async () => {
      latest = await readProfileState(profileId);
      return latest === expected;
    },
    {
      timeout: timeoutMs,
      interval: 500,
      timeoutMsg: `${caseMarker}: profile ${profileId} 状态没有收敛到 ${expected}，latest=${latest}`,
    },
  );
}

/** 等待 Profile 离开指定状态（用于证明停服后 Rust Host 确实观察到断线）。 */
export async function waitForProfileStateNot(
  caseMarker: string,
  profileId: string,
  unexpected: string,
  timeoutMs = 60_000,
): Promise<void> {
  let latest: string | null = null;
  await browser.waitUntil(
    async () => {
      latest = await readProfileState(profileId);
      return latest !== null && latest !== unexpected;
    },
    {
      timeout: timeoutMs,
      interval: 500,
      timeoutMsg: `${caseMarker}: profile ${profileId} 停服后仍停留在 ${unexpected}，latest=${latest}`,
    },
  );
}

async function readProfileState(profileId: string): Promise<string | null> {
  return browser.executeAsync((wantedProfileId, done) => {
    const services = (
      window as Window & {
        __remoteWorkspaceSessionStoreE2E?: {
          getState?: () => {
            baseServices?: {
              sourceServerManagementService?: {
                getSnapshot: () => Promise<{
                  states: Array<{ profileId: string; state: string }>;
                }>;
              };
            } | null;
          };
        };
      }
    ).__remoteWorkspaceSessionStoreE2E?.getState?.().baseServices;
    const service = services?.sourceServerManagementService;
    if (!service) {
      done(null);
      return;
    }
    void service.getSnapshot().then(
      (snapshot) => {
        done(snapshot.states.find((state) => state.profileId === wantedProfileId)?.state ?? null);
      },
      () => done(null),
    );
  }, profileId);
}
