import { execFile } from "node:child_process";
import { mkdtemp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { pathToFileURL } from "node:url";
import { promisify } from "node:util";
import {
  TID_CONFIRM_DIALOG_CONFIRM,
  TID_SVN_BRANCH_SWITCHER,
  TID_SVN_COMMIT_BUTTON,
  TID_SVN_COMMIT_DIALOG,
  TID_SVN_COMMIT_MESSAGE_INPUT,
  TID_SVN_COMMIT_SUBMIT,
  TID_SVN_MERGE_BUTTON,
  TID_SVN_MERGE_DIALOG,
  TID_SVN_MERGE_SUBMIT,
  TID_SVN_PANE,
  TID_SVN_REFRESH_BUTTON,
  TID_SVN_STATUS_CHANGES,
  TID_SVN_UPDATE_BUTTON,
  TID_V4_SESSION_PANE,
  TID_V4_STOP,
} from "@escode/shared";
import { DEFAULT_WORKSPACE, clearAppData, waitForWorkspaceApp } from "../../helpers/desktop-app.js";
import {
  E2E_REPLY_TOKEN,
  buildReadonlyToolPrompt,
  ensureReadonlyToolFixtureFile,
} from "../../helpers/conversation-session.js";
import { sel } from "../../helpers/selectors.js";
import {
  prepareV4ConversationE2E,
  sendV4Prompt,
  waitForV4TimelineContaining,
} from "../../helpers/v4-conversation.js";

/**
 * SVN 交互验收（spec：docs/svn/requirements.md「测试与验收」、docs/svn/ui.md §12）：
 * - SVN-UI-01 修改 → 勾选 → 提交（含自动 add 未纳入版本控制的文件）
 * - SVN-UI-02 更新产生冲突 → 用我的 → 提交
 * - SVN-UI-03 切换分支
 * - SVN-UI-04 合并分支改动 → 提交（mergeinfo 写入服务器）
 * 需要运行环境里有 svn / svnadmin（可用 ESCODE_SVN_BINARY / ESCODE_SVNADMIN_BINARY 指定）；
 * 缺失时显式跳过并输出原因。
 */
const TEST_TIMEOUT_MS = 180000;
const TID_CHAT_SUMMARY_PANEL = "chat-summary-panel";
const TRACKED_FILE = join(".escode-e2e", "svn", "tracked.md");
const NEW_FILE = join(".escode-e2e", "svn", "new.md");
const MERGED_FILE = join(".escode-e2e", "svn", "merged.md");
const execFileAsync = promisify(execFile);

const svnBinary = process.env.ESCODE_SVN_BINARY?.trim() || "svn";
const svnadminBinary = process.env.ESCODE_SVNADMIN_BINARY?.trim() || "svnadmin";

async function svn(cwd: string, ...args: string[]): Promise<string> {
  const { stdout } = await execFileAsync(svnBinary, ["--non-interactive", ...args], {
    cwd,
    env: { ...process.env, LC_MESSAGES: "C" },
  });
  return stdout;
}

async function svnAvailable(): Promise<boolean> {
  try {
    await execFileAsync(svnBinary, ["--version", "--quiet"]);
    await execFileAsync(svnadminBinary, ["--version", "--quiet"]);
    return true;
  } catch {
    return false;
  }
}

interface SvnFixture {
  repoUrl: string;
  otherWc: string;
  branchWc: string;
}

/** 在 DEFAULT_WORKSPACE 上建立 trunk 工作副本，并准备 branches/feature 与第二个工作副本。 */
async function prepareSvnFixture(): Promise<SvnFixture> {
  const root = await mkdtemp(join(tmpdir(), "escode-svn-e2e-"));
  await execFileAsync(svnadminBinary, ["create", join(root, "repo")]);
  const repoUrl = pathToFileURL(join(root, "repo")).href;
  await svn(root, "mkdir", "-m", "layout", `${repoUrl}/trunk`, `${repoUrl}/branches`);
  // 同层同时存在 .git 时按规则优先 Git；SVN 用例必须移除其他用例遗留的 .git。
  await rm(join(DEFAULT_WORKSPACE, ".git"), { recursive: true, force: true });
  await mkdir(DEFAULT_WORKSPACE, { recursive: true });
  await svn(root, "checkout", "-q", "--force", `${repoUrl}/trunk`, DEFAULT_WORKSPACE);
  const tracked = join(DEFAULT_WORKSPACE, TRACKED_FILE);
  await mkdir(dirname(tracked), { recursive: true });
  await writeFile(tracked, "line one\nline two\n", "utf8");
  await svn(DEFAULT_WORKSPACE, "add", "-q", "--parents", TRACKED_FILE);
  await svn(DEFAULT_WORKSPACE, "commit", "-q", "-m", "test: svn baseline", ".escode-e2e");
  await svn(root, "copy", "-m", "branch", `${repoUrl}/trunk`, `${repoUrl}/branches/feature`);
  await svn(DEFAULT_WORKSPACE, "update", "-q");
  const otherWc = join(root, "other");
  await svn(root, "checkout", "-q", `${repoUrl}/trunk`, otherWc);
  const branchWc = join(root, "feature");
  await svn(root, "checkout", "-q", `${repoUrl}/branches/feature`, branchWc);
  return { repoUrl, otherWc, branchWc };
}

async function createActiveTask() {
  await ensureReadonlyToolFixtureFile();
  await prepareV4ConversationE2E();
  const marker = `E2E_SVN_WORKFLOW_${Date.now()}`;
  await sendV4Prompt(buildReadonlyToolPrompt(marker));
  await waitForV4TimelineContaining(marker);
  await waitForV4TimelineContaining(E2E_REPLY_TOKEN, 90000);
  // 任务落盘后 pane 可能以任务 tab 的 id 重新挂载；按前缀找任意 v4 pane，
  // 只要求已存在且没有运行中的 stop 按钮，避免绑定固定的 workspace-main。
  await browser.waitUntil(
    async () =>
      (await browser.execute(
        (panePrefix: string, stopTestId: string) => {
          const panes = Array.from(
            document.querySelectorAll<HTMLElement>(`[data-testid^="${panePrefix}"]`),
          );
          return (
            panes.length > 0 &&
            !panes.some((pane) => Boolean(pane.querySelector(`[data-testid="${stopTestId}"]`)))
          );
        },
        TID_V4_SESSION_PANE,
        TID_V4_STOP,
      )) as boolean,
    { timeout: 90000, timeoutMsg: "SVN E2E 的 v4 session 没有进入空闲状态" },
  );
  await $(sel(TID_CHAT_SUMMARY_PANEL)).waitForDisplayed({
    timeout: 30000,
    timeoutMsg: "SVN E2E 没有显示摘要面板",
  });
}

/** 通过 DOM click 触发：状态面板 container query 切换时 WebDriver 可能报告 0×0 尺寸。 */
async function clickTestId(testId: string, timeoutMsg: string) {
  await browser.waitUntil(
    async () =>
      (await browser.execute((id: string) => {
        const element = Array.from(
          document.querySelectorAll<HTMLButtonElement>(`[data-testid="${id}"]`),
        ).find((candidate) => !candidate.disabled);
        element?.click();
        return Boolean(element);
      }, testId)) as boolean,
    { timeout: 30000, timeoutMsg },
  );
}

async function openSvnPane() {
  await clickTestId(TID_SVN_STATUS_CHANGES, "SVN 改动入口没有出现");
  await $(sel(TID_SVN_PANE)).waitForDisplayed({ timeout: 30000, timeoutMsg: "SVN 面板没有打开" });
}

async function waitForChangeRow(path: string, present = true) {
  const normalized = path.replace(/\\/g, "/");
  await browser.waitUntil(
    async () =>
      ((await browser.execute(
        (target: string) => Boolean(document.querySelector(`[data-svn-change-path="${target}"]`)),
        normalized,
      )) as boolean) === present,
    {
      // 自动刷新有防抖；操作后由面板主动刷新，这里只需等待一次读取完成。
      timeout: 60000,
      timeoutMsg: `SVN 面板中 ${normalized} ${present ? "没有出现" : "没有消失"}`,
    },
  );
}

async function setRowChecked(path: string, checked: boolean) {
  await browser.waitUntil(
    async () =>
      (await browser.execute(
        (target: string, nextChecked: boolean) => {
          const checkbox = document.querySelector<HTMLButtonElement>(
            `[data-svn-change-path="${target}"] [data-slot="checkbox"]`,
          );
          if (!checkbox) {
            return false;
          }
          if ((checkbox.getAttribute("data-state") === "checked") !== nextChecked) {
            checkbox.click();
            return false;
          }
          return true;
        },
        path.replace(/\\/g, "/"),
        checked,
      )) as boolean,
    { timeout: 15000, timeoutMsg: `${path} 的勾选状态没有变为 ${checked}` },
  );
}

/** unversioned 分组默认折叠，必须先展开才能操作行内勾选框。 */
async function expandGroup(groupId: string) {
  await browser.waitUntil(
    async () =>
      (await browser.execute((id: string) => {
        const header = document.querySelector<HTMLButtonElement>(
          `[data-svn-group="${id}"] button[aria-expanded]`,
        );
        if (!header) {
          return false;
        }
        if (header.getAttribute("aria-expanded") !== "true") {
          header.click();
          return false;
        }
        return true;
      }, groupId)) as boolean,
    { timeout: 15000, timeoutMsg: `SVN 分组 ${groupId} 没有展开` },
  );
}

async function clickRowButton(path: string, labels: readonly string[]) {
  await browser.waitUntil(
    async () =>
      (await browser.execute(
        (target: string, candidates: string[]) => {
          const row = document.querySelector(`[data-svn-change-path="${target}"]`);
          const button = Array.from(row?.querySelectorAll<HTMLButtonElement>("button") ?? []).find(
            (element) => !element.disabled && candidates.includes(element.innerText.trim()),
          );
          button?.click();
          return Boolean(button);
        },
        path.replace(/\\/g, "/"),
        [...labels],
      )) as boolean,
    { timeout: 30000, timeoutMsg: `${path} 行内没有找到按钮 ${labels.join("/")}` },
  );
}

async function confirmAlert(labels: readonly string[]) {
  await browser.waitUntil(
    async () =>
      (await browser.execute(
        (confirmTestId: string, candidates: string[]) => {
          const button = document.querySelector<HTMLButtonElement>(
            `[data-testid="${confirmTestId}"]`,
          );
          if (!button || button.disabled) {
            return false;
          }
          if (!candidates.some((label) => (button.innerText ?? "").includes(label))) {
            return false;
          }
          button.click();
          return true;
        },
        TID_CONFIRM_DIALOG_CONFIRM,
        [...labels],
      )) as boolean,
    { timeout: 15000, timeoutMsg: `确认弹窗中没有找到 ${labels.join("/")}` },
  );
}

async function commitFromPane(message: string) {
  await clickTestId(TID_SVN_COMMIT_BUTTON, "SVN 提交按钮不可用");
  await $(sel(TID_SVN_COMMIT_DIALOG)).waitForDisplayed({ timeout: 15000 });
  await $(sel(TID_SVN_COMMIT_MESSAGE_INPUT)).setValue(message);
  await clickTestId(TID_SVN_COMMIT_SUBMIT, "SVN 提交确认按钮不可用");
  await $(sel(TID_SVN_COMMIT_DIALOG)).waitForDisplayed({
    reverse: true,
    timeout: 60000,
    timeoutMsg: "SVN 提交弹窗没有在提交后关闭",
  });
}

async function switchBranchFromUi(label: string, expectedRelativeUrl: string) {
  await clickTestId(TID_SVN_BRANCH_SWITCHER, "SVN 分支入口没有出现");
  await browser.waitUntil(
    async () =>
      (await browser.execute((target: string) => {
        const option = Array.from(
          document.querySelectorAll<HTMLButtonElement>(
            "[data-radix-popper-content-wrapper] button",
          ),
        ).find((element) => element.innerText.trim() === target && !element.disabled);
        option?.click();
        return Boolean(option);
      }, label)) as boolean,
    { timeout: 30000, timeoutMsg: `分支列表中没有 ${label}` },
  );
  await browser.waitUntil(
    async () =>
      (await svn(DEFAULT_WORKSPACE, "info", "--show-item", "relative-url")).trim() ===
      expectedRelativeUrl,
    { timeout: 60000, timeoutMsg: `工作副本没有切换到 ${expectedRelativeUrl}` },
  );
}

describe("SVN 工作流 UI manual-review E2E", () => {
  let fixture: SvnFixture | null = null;

  before(async function () {
    if (!(await svnAvailable())) {
      console.warn("[svn-workflow-ui-flow] 跳过：运行环境没有 svn / svnadmin");
      this.skip();
    }
    await waitForWorkspaceApp(DEFAULT_WORKSPACE, 30000);
    fixture = await prepareSvnFixture();
    // VCS 判定只在 workspace 挂载时读取一次；fixture 在 app 启动后才创建 .svn，
    // 必须重载 renderer 才能让 useWorkspaceVcs 识别为 SVN 工作区。
    await browser.refresh();
    await waitForWorkspaceApp(DEFAULT_WORKSPACE, 30000);
    await createActiveTask();
  });

  after(async () => {
    await browser.electron.restoreAllMocks();
    await clearAppData();
  });

  it("SVN-UI-01: 修改并新增文件后勾选提交，新文件自动 add 并写入服务器", async function () {
    this.timeout(TEST_TIMEOUT_MS);
    await writeFile(join(DEFAULT_WORKSPACE, TRACKED_FILE), "line one\nchanged\n", "utf8");
    await writeFile(join(DEFAULT_WORKSPACE, NEW_FILE), "brand new\n", "utf8");
    await openSvnPane();
    // 自动刷新防抖较长，打开面板后手动刷新一次以读取刚写入的改动。
    await clickTestId(TID_SVN_REFRESH_BUTTON, "SVN 刷新按钮不可用");
    await waitForChangeRow(TRACKED_FILE);
    // 未纳入版本控制的文件默认折叠，先展开分组再勾选。
    await expandGroup("unversioned");
    await waitForChangeRow(NEW_FILE);
    await setRowChecked(NEW_FILE, true);
    await commitFromPane("test(svn): commit selected files");
    await waitForChangeRow(TRACKED_FILE, false);
    await waitForChangeRow(NEW_FILE, false);
    const log = await svn(DEFAULT_WORKSPACE, "log", "-v", "-l", "1", `${fixture!.repoUrl}/trunk`);
    expect(log).toContain("test(svn): commit selected files");
    expect(log).toContain("new.md");
  });

  it("SVN-UI-02: 更新产生冲突后提交被禁用，选择用我的后可以提交", async function () {
    this.timeout(TEST_TIMEOUT_MS);
    await svn(fixture!.otherWc, "update", "-q");
    await writeFile(join(fixture!.otherWc, TRACKED_FILE), "line one\ntheirs\n", "utf8");
    await svn(fixture!.otherWc, "commit", "-q", "-m", "theirs");
    await writeFile(join(DEFAULT_WORKSPACE, TRACKED_FILE), "line one\nmine\n", "utf8");

    await clickTestId(TID_SVN_UPDATE_BUTTON, "SVN 更新按钮不可用");
    await $('[data-svn-group="conflicted"]').waitForExist({
      timeout: 60000,
      timeoutMsg: "更新后没有出现冲突分组",
    });
    expect(await $(sel(TID_SVN_COMMIT_BUTTON)).isEnabled()).toBe(false);

    await clickRowButton(TRACKED_FILE, ["用我的", "Use mine"]);
    await confirmAlert(["用我的", "Use mine"]);
    await $('[data-svn-group="conflicted"]').waitForExist({ reverse: true, timeout: 60000 });
    expect(await readFile(join(DEFAULT_WORKSPACE, TRACKED_FILE), "utf8")).toBe("line one\nmine\n");
    await commitFromPane("fix(svn): keep mine");
    await waitForChangeRow(TRACKED_FILE, false);
  });

  it("SVN-UI-03: 通过分支入口切换到 branches/feature", async function () {
    this.timeout(TEST_TIMEOUT_MS);
    await clickTestId(TID_SVN_BRANCH_SWITCHER, "SVN 分支入口没有出现");
    await browser.waitUntil(
      async () =>
        (await browser.execute(() => {
          const option = Array.from(
            document.querySelectorAll<HTMLButtonElement>(
              "[data-radix-popper-content-wrapper] button",
            ),
          ).find((element) => element.innerText.trim() === "feature" && !element.disabled);
          option?.click();
          return Boolean(option);
        })) as boolean,
      { timeout: 30000, timeoutMsg: "分支列表中没有 feature" },
    );
    await browser.waitUntil(
      async () =>
        (await svn(DEFAULT_WORKSPACE, "info", "--show-item", "relative-url")).trim() ===
        "^/branches/feature",
      { timeout: 60000, timeoutMsg: "工作副本没有切换到 branches/feature" },
    );
    await browser.waitUntil(
      async () =>
        (
          (await browser.execute(
            (testId: string) =>
              document.querySelector(`[data-testid="${testId}"]`)?.textContent ?? "",
            TID_SVN_BRANCH_SWITCHER,
          )) as string
        ).includes("feature"),
      { timeout: 30000, timeoutMsg: "分支入口没有显示 feature" },
    );
  });

  it("SVN-UI-04: 通过合并弹窗把 feature 的改动合并到 trunk 并提交", async function () {
    this.timeout(TEST_TIMEOUT_MS);
    await switchBranchFromUi("trunk", "^/trunk");

    // 在 feature 分支上提交一个新文件，作为合并来源。
    await svn(fixture!.branchWc, "update", "-q");
    await writeFile(join(fixture!.branchWc, MERGED_FILE), "merged from feature\n", "utf8");
    await svn(fixture!.branchWc, "add", "-q", "--parents", MERGED_FILE);
    await svn(fixture!.branchWc, "commit", "-q", "-m", "test: feature merge source");

    await clickTestId(TID_SVN_MERGE_BUTTON, "SVN 合并按钮不可用");
    await $(sel(TID_SVN_MERGE_DIALOG)).waitForDisplayed({ timeout: 15000 });
    // 分支列表异步加载；加载完成且自动选中 feature 后确认按钮才可用。
    await browser.waitUntil(async () => await $(sel(TID_SVN_MERGE_SUBMIT)).isEnabled(), {
      timeout: 30000,
      timeoutMsg: "合并弹窗的分支列表没有加载完成",
    });
    await clickTestId(TID_SVN_MERGE_SUBMIT, "SVN 合并确认按钮不可用");
    await $(sel(TID_SVN_MERGE_DIALOG)).waitForDisplayed({
      reverse: true,
      timeout: 60000,
      timeoutMsg: "SVN 合并弹窗没有在合并后关闭",
    });
    await waitForChangeRow(MERGED_FILE);

    await commitFromPane("feat(svn): merge feature branch");
    await waitForChangeRow(MERGED_FILE, false);
    const log = await svn(DEFAULT_WORKSPACE, "log", "-v", "-l", "1", `${fixture!.repoUrl}/trunk`);
    expect(log).toContain("feat(svn): merge feature branch");
    expect(log).toContain("merged.md");
    expect(await svn(DEFAULT_WORKSPACE, "propget", "svn:mergeinfo", ".")).toContain(
      "branches/feature",
    );
  });
});
