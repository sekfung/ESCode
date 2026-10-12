import { readFile } from "node:fs/promises";
import {
  TID_SETTINGS_PAGE,
  TID_SETTINGS_SECTION_NAV,
  TID_TASK_SETTINGS_BUTTON,
  testId,
} from "@escode/shared";
import {
  clearAppData,
  clickTestIdByDom,
  getE2EAppDataPaths,
  waitForDefaultWorkspaceReady,
  waitForTestIdByDom,
} from "../../helpers/desktop-app.js";

const OAUTH_CREDENTIAL_KEYS = [
  "oauth:active_provider",
  "oauth:zai:access_token",
  "oauth:zai:user_info",
  "escodejwttoken",
] as const;

// “不应该发生”的断言需要留出广播送达并可能触发 UI 的窗口，取足够大的收敛时间。
const FORBIDDEN_REACTION_SETTLE_MS = 8000;

// 免登录发行版回归锚点：运行中官方 JWT 失效仍然清理失效凭据，
// 但不再提示“登录已过期”、也不再重启应用进入登录流程。产品规则见 docs/ui/login-page.md。
describe("ESCode JWT 401 不触发强制重新登录", () => {
  after(async () => {
    await browser.electron.restoreAllMocks();
    await clearAppData();
  });

  it("运行中收到普通 401 后清理凭据，但不提示登录过期也不重启", async function () {
    this.timeout(120_000);

    await waitForDefaultWorkspaceReady(60_000);

    const networkMock = await browser.electron.mock("net", "fetch");
    await networkMock.mockResolvedValue({ ok: false, status: 401 });
    const relaunchMock = await browser.electron.mock("app", "relaunch");
    await relaunchMock.mockResolvedValue(undefined);
    const quitMock = await browser.electron.mock("app", "quit");
    await quitMock.mockResolvedValue(undefined);

    // 打开模型供应商页会触发一次 getAll；mock 从此处开始，避免启动阶段的匿名请求抢先结束用例。
    await clickTestIdByDom(TID_TASK_SETTINGS_BUTTON, {
      timeout: 15000,
      timeoutMsg: "没有找到设置入口按钮",
    });
    await waitForTestIdByDom(TID_SETTINGS_PAGE, {
      timeout: 15000,
      timeoutMsg: "设置页没有出现",
    });
    await clickTestIdByDom(testId(TID_SETTINGS_SECTION_NAV, "modelProvider"), {
      timeout: 15000,
      timeoutMsg: "设置页没有出现模型供应商分区入口",
    });

    // Host 侧失效观察与凭据清理不属于“强制登录入口”，必须继续执行。
    await waitForOAuthCredentialsCleanup();
    expect(networkMock.mock.calls.length).toBeGreaterThan(0);

    // 自动登录入口关闭：既不弹“登录已过期”，也不发出重启/退出命令。
    await browser.pause(FORBIDDEN_REACTION_SETTLE_MS);
    expect(await hasExpiredLoginAlert()).toBe(false);
    expect(relaunchMock.mock.calls.length).toBe(0);
    expect(quitMock.mock.calls.length).toBe(0);
  });
});

async function hasExpiredLoginAlert(): Promise<boolean> {
  return (await browser.execute(() => {
    const content = document.querySelector<HTMLElement>('[data-slot="alert-dialog-content"]');
    return /登录已过期|Your session has expired/u.test(
      content?.querySelector<HTMLElement>('[data-slot="alert-dialog-title"]')?.textContent ?? "",
    );
  })) as boolean;
}

async function waitForOAuthCredentialsCleanup(): Promise<void> {
  await browser.waitUntil(
    async () => {
      try {
        const credentials = JSON.parse(
          await readFile(getE2EAppDataPaths().credentialsFile, "utf-8"),
        ) as Record<string, string>;
        return OAUTH_CREDENTIAL_KEYS.every((key) => !(key in credentials));
      } catch {
        return false;
      }
    },
    {
      timeout: 10000,
      timeoutMsg: "401 后 OAuth/JWT 凭据没有清理",
    },
  );
}
