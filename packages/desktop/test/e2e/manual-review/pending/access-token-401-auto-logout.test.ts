import { readFile } from "node:fs/promises";
import { DesktopCommandIds, PlatformChannels } from "@escode/shared";
import { clearAppData, getE2EAppDataPaths } from "../../helpers/desktop-app.js";

const responseStatus = process.env.ESCODE_E2E_AUTH_EXPIRY_HTTP_STATUS === "200" ? 200 : 401;

// “不应该发生”的断言需要留出广播送达并可能触发 UI 的窗口，取足够大的收敛时间。
const FORBIDDEN_REACTION_SETTLE_MS = 8000;

// 免登录发行版回归锚点：运行中业务 access token 失效仍然清理当前登录状态，
// 但不再弹“登录已过期”、也不再重启应用进入登录流程。产品规则见 docs/ui/login-page.md。
describe("用户资料 access token 登录过期", () => {
  after(async () => {
    await browser.electron.restoreAllMocks();
    await clearAppData();
  });

  it(`AUTH-06: 真实 Host 请求 HTTP ${responseStatus} + 业务 401 后清理当前登录，但不弹过期提示也不重启`, async () => {
    const baseUrl = process.env.ESCODE_E2E_ACCESS_TOKEN_401_URL;
    if (!baseUrl) throw new Error("缺少隔离 userinfo HTTP fixture");

    await browser.waitUntil(
      async () => {
        const response = await fetch(`${baseUrl}/__e2e/status`);
        const status = (await response.json()) as { businessRequestCount: number };
        return status.businessRequestCount > 0;
      },
      { timeout: 30_000, timeoutMsg: "Host 没有使用业务 access token 请求 userinfo" },
    );
    const before = await readCredentials();
    expect(before["oauth:active_provider"]).toBe("bigmodel");
    expect(await expiredAlertCount()).toBe(0);

    await fetch(`${baseUrl}/__e2e/reject?status=${responseStatus}`, { method: "POST" });
    const fixtureStatus = (await (await fetch(`${baseUrl}/__e2e/status`)).json()) as {
      rejectionStatus: number;
      rejected: boolean;
    };
    expect(fixtureStatus.rejectionStatus).toBe(responseStatus);
    expect(fixtureStatus.rejected).toBe(true);

    // Host 侧失效清理与“是否强制重新登录”无关，必须继续执行。
    await browser.waitUntil(
      async () => {
        const credentials = await readCredentials();
        return [
          "oauth:active_provider",
          "oauth:bigmodel:access_token",
          "oauth:bigmodel:refresh_token",
          "oauth:bigmodel:user_info",
          "escodejwttoken",
        ].every((key) => !(key in credentials));
      },
      { timeout: 10_000, timeoutMsg: "业务 401 后没有完成原 OAuth 清理" },
    );
    // 自动登录入口关闭：不弹“登录已过期”，也不发出 RelaunchApp。
    // 测试原因：仅 mock app.relaunch/quit 仍会先执行真实 Host 关闭屏障，导致
    // ChromeDriver 收尾超时，因此在 IPC 边界记录命令而不实际重启。
    await browser.electron.execute((electron, channel) => {
      const state = globalThis as typeof globalThis & { __oauth401Commands?: string[] };
      state.__oauth401Commands = [];
      electron.ipcMain.removeHandler(channel);
      electron.ipcMain.handle(channel, (_event, command: string) => {
        state.__oauth401Commands?.push(command);
      });
    }, PlatformChannels.ExecuteDesktopCommand);

    await browser.pause(FORBIDDEN_REACTION_SETTLE_MS);
    expect(await expiredAlertCount()).toBe(0);
    const commands = await browser.electron.execute(
      () =>
        (globalThis as typeof globalThis & { __oauth401Commands?: string[] }).__oauth401Commands ??
        [],
    );
    expect(commands).not.toContain(DesktopCommandIds.RelaunchApp);
  });
});

async function readCredentials(): Promise<Record<string, string>> {
  return JSON.parse(await readFile(getE2EAppDataPaths().credentialsFile, "utf8"));
}

async function expiredAlertCount(): Promise<number> {
  return browser.execute(
    () =>
      Array.from(document.querySelectorAll('[data-slot="alert-dialog-title"]')).filter((element) =>
        /登录已过期|Your session has expired/u.test(element.textContent ?? ""),
      ).length,
  );
}
