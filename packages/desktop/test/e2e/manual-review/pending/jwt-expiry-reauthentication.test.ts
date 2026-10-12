import { readFile } from "node:fs/promises";
import { TID_LOGIN_TRIGGER, TID_OAUTH_LOGIN_BUTTON, testId, ZAI_PROVIDER_ID } from "@escode/shared";
import {
  clearAppData,
  getE2EAppDataPaths,
  waitForDefaultWorkspaceReady,
} from "../../helpers/desktop-app.js";

const EXPIRED_SESSION_CREDENTIAL_KEYS = [
  "oauth:active_provider",
  `oauth:${ZAI_PROVIDER_ID}:access_token`,
  `oauth:${ZAI_PROVIDER_ID}:refresh_token`,
  `oauth:${ZAI_PROVIDER_ID}:user_info`,
  "escodejwttoken",
] as const;

// 免登录发行版回归锚点：官方 JWT 失效只清理失效登录态，不再提示重新登录、
// 也不再把用户从主界面带到登录入口。产品规则见 docs/ui/login-page.md。
describe("ESCode JWT 过期后的免登录冷启动", () => {
  after(async () => {
    await browser.electron.restoreAllMocks();
    await clearAppData();
  });

  it("冷启动清理过期登录态后直接进入主界面，不弹登录过期提示也不进登录页", async () => {
    await waitForDefaultWorkspaceReady(60000);

    // 认证事实必须在启动阶段被清理，否则主界面会带着过期 JWT 发请求。
    await waitForExpiredSessionCredentialsCleanup();

    // 提醒与强制登录入口属于“自动登录入口”，免登录发行版必须全部关闭。
    await expectNoExpiredLoginAlert();
    await expectNoLoginChannelEntry();

    // 登录代码本身保留：主界面仍提供显式登录入口。
    await expectVisibleLoginTrigger();
  });
});

async function expectNoExpiredLoginAlert() {
  const snapshot = await readAlertSnapshot();
  expect(snapshot.title).not.toMatch(/登录已过期|Your session has expired/u);
}

async function readAlertSnapshot(): Promise<{ title: string; description: string }> {
  return (await browser.execute(() => {
    const content = document.querySelector<HTMLElement>('[data-slot="alert-dialog-content"]');
    return {
      title:
        content
          ?.querySelector<HTMLElement>('[data-slot="alert-dialog-title"]')
          ?.textContent?.trim() ?? "",
      description:
        content
          ?.querySelector<HTMLElement>('[data-slot="alert-dialog-description"]')
          ?.textContent?.trim() ?? "",
    };
  })) as { title: string; description: string };
}

async function expectNoLoginChannelEntry() {
  const hasZaiLoginButton = (await browser.execute(
    (zaiLoginButtonId: string) => {
      return Boolean(document.querySelector(`[data-testid="${zaiLoginButtonId}"]`));
    },
    testId(TID_OAUTH_LOGIN_BUTTON, ZAI_PROVIDER_ID),
  )) as boolean;

  expect(hasZaiLoginButton).toBe(false);
}

async function expectVisibleLoginTrigger() {
  await browser.waitUntil(
    async () =>
      (await browser.execute((loginTriggerId: string) => {
        const element = Array.from(
          document.querySelectorAll<HTMLElement>(`[data-testid="${loginTriggerId}"]`),
        ).find((candidate) => candidate.getClientRects().length > 0);
        return Boolean(element);
      }, TID_LOGIN_TRIGGER)) as boolean,
    {
      timeout: 20000,
      timeoutMsg: "免登录冷启动后主界面没有保留显式登录入口",
    },
  );
}

async function waitForExpiredSessionCredentialsCleanup() {
  try {
    await browser.waitUntil(
      async () => {
        const credentials = await readCredentials();
        return EXPIRED_SESSION_CREDENTIAL_KEYS.every((key) => !(key in credentials));
      },
      {
        timeout: 10000,
        timeoutMsg: "过期认证凭据没有被清理",
      },
    );
  } catch (error) {
    const credentials = await readCredentials().catch(() => null);
    const keys = credentials ? Object.keys(credentials).sort().join(", ") : "<missing>";
    throw new Error(`过期认证凭据没有被清理，当前凭据 key: ${keys}`, {
      cause: error,
    });
  }
}

async function readCredentials(): Promise<Record<string, string>> {
  const { credentialsFile } = getE2EAppDataPaths();
  return JSON.parse(await readFile(credentialsFile, "utf-8")) as Record<string, string>;
}
