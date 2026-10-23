import { prepareV4ConversationE2E } from "../helpers/v4-conversation.js";
import { readHostProcesses } from "../helpers/ssh-remote-p0.js";
import { clearAppData } from "../helpers/desktop-app.js";

// docs/specs/rust-u2-window-host.md §6：Rust Host 承载本地窗口时的图形会话验收。
// 多窗口 attachment 共用同一 Host 的集成覆盖在 rustHostResilience.test.ts；这里验证
// 产品窗口的真实进程拓扑与 renderer reload 后 Host 进程不变。
// M5 默认走 Rust Host；显式 0/false 才回到 Node Host。
const RUST_MODE = !["0", "false"].includes(
  (process.env.ESCODE_RUST_HOST ?? "").trim().toLowerCase(),
);
const CASE_MARKER = "RUST_HOST_WINDOW";

describe(`${CASE_MARKER}: Rust Host 窗口生命周期`, () => {
  if (!RUST_MODE) {
    it.skip("需要 ESCODE_RUST_HOST=1", () => undefined);
    return;
  }

  after(async () => {
    await browser.electron.restoreAllMocks().catch(() => undefined);
    await clearAppData().catch(() => undefined);
  });

  it("本地窗口只由一个 Rust Host 服务，renderer reload 保持同一 pid", async function () {
    this.timeout(5 * 60_000);
    await prepareV4ConversationE2E({ skipProvider: true });

    const initial = await readHostProcesses();
    const rustHosts = initial.filter((process) => process.name === "escode-host-rust-host");
    expect(rustHosts).toHaveLength(1);
    // 本地窗口不应再出现 utility Node Host（escode-host-local-*）。
    expect(initial).toEqual(rustHosts);
    const hostPid = rustHosts[0]!.pid;

    await browser.execute(() => window.location.reload());
    await browser.waitUntil(
      () =>
        browser.execute(() =>
          Boolean(
            document.querySelector('[data-testid="task-settings-button"]') ||
            document.querySelector('[data-testid="onboarding-page"]'),
          ),
        ),
      { timeout: 60_000, interval: 500, timeoutMsg: "reload 后工作区没有恢复" },
    );

    const afterReload = await readHostProcesses();
    expect(afterReload).toEqual([{ name: "escode-host-rust-host", pid: hostPid }]);
  });
});
