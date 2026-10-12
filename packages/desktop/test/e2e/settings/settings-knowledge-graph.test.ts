import { mkdir, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { homedir } from "node:os";
import { join } from "node:path";
import {
  TID_CONFIRM_DIALOG_CONFIRM,
  TID_SETTINGS_PAGE,
  TID_SETTINGS_SECTION_NAV,
  TID_TASK_SETTINGS_BUTTON,
  testId,
} from "@escode/shared";
import {
  DEFAULT_WORKSPACE,
  clearAppData,
  clickTestIdByDom,
  waitForDefaultWorkspaceReady,
  waitForWorkspaceApp,
} from "../helpers/desktop-app.js";
import { skipOccupationOnboardingIfPresent } from "../helpers/occupation-onboarding.js";
import { sel } from "../helpers/selectors.js";

// 设置 →「知识库」（apps/escode-cli-rust/docs/llm-wiki/01-spec.md §11）。
// - Node runtime（开发态 E2E 默认）：页面必须明确显示「当前 runtime 不支持」，不渲染图谱。
// - Rust runtime（ESCODE_AGENT_SERVER_RUNTIME=escode-cli-rust）：预置一条全局候选知识，
//   在图谱中选中 → 批准 → 确认框确认 → 条目文件状态落盘为 approved 且详情刷新。
// 全局作用域不依赖项目身份哈希，预置文件位置只取决于存储目录。

const ENTRY_ID = "e2e-knowledge-error-envelope";
const ENTRY_TITLE = "E2E knowledge error envelope";

async function appEnv(name: string): Promise<string | undefined> {
  return browser.electron.execute((_electron, key: string) => process.env[key], name);
}

/** 与 Rust runtime 的 settings::load 一致：ESCODE_STORAGE_DIR 优先，缺省 ~/.escode；Global Wiki 根为 <storage>/wiki。 */
async function wikiGlobalEntriesDir(): Promise<string> {
  const storage = (await appEnv("ESCODE_STORAGE_DIR"))?.trim() || join(homedir(), ".escode");
  return join(storage, "wiki", "entries");
}

async function seedGlobalProposal(): Promise<string> {
  const dir = await wikiGlobalEntriesDir();
  await mkdir(dir, { recursive: true });
  const file = join(dir, `${ENTRY_ID}.md`);
  await writeFile(
    file,
    [
      "---",
      "schema_version: 1",
      `id: ${ENTRY_ID}`,
      `title: ${ENTRY_TITLE}`,
      "scope: global",
      "kind: contract",
      "status: proposal",
      "authority: proposal",
      "origin: user-authored",
      "validity:",
      "  state: valid",
      "tags: [e2e]",
      "created_at: 2026-10-01T00:00:00Z",
      "updated_at: 2026-10-01T00:00:00Z",
      "---",
      "API errors use a `{ code, message }` envelope.",
      "",
    ].join("\n"),
    "utf-8",
  );
  return file;
}

/** KG-02 在 KG-01 之后运行：设置页可能仍打开，此时只切换分区。 */
async function ensureKnowledgeSettings() {
  if (
    await $(sel(TID_SETTINGS_PAGE))
      .isDisplayed()
      .catch(() => false)
  ) {
    await clickTestIdByDom(testId(TID_SETTINGS_SECTION_NAV, "knowledge"), {
      timeout: 15000,
      timeoutMsg: "设置页没有出现「知识库」分区入口",
    });
    await $(sel("knowledge-settings-section")).waitForDisplayed({ timeout: 15000 });
    return;
  }
  await openKnowledgeSettings();
}

async function openKnowledgeSettings() {
  await clickTestIdByDom(TID_TASK_SETTINGS_BUTTON, {
    timeout: 15000,
    timeoutMsg: "没有找到设置入口按钮",
  });
  await $(sel(TID_SETTINGS_PAGE)).waitForDisplayed({ timeout: 15000 });
  await clickTestIdByDom(testId(TID_SETTINGS_SECTION_NAV, "knowledge"), {
    timeout: 15000,
    timeoutMsg: "设置页没有出现「知识库」分区入口",
  });
  await $(sel("knowledge-settings-section")).waitForDisplayed({ timeout: 15000 });
}

describe("设置 · 知识库图谱 E2E", () => {
  let seededFile: string | null = null;

  after(async () => {
    if (seededFile) await rm(seededFile, { force: true });
    await clearAppData();
  });

  it("KG-01: 按 runtime 能力显示不支持态，或在图谱中审核候选知识并落盘", async function () {
    this.timeout(120000);
    const rust = (await appEnv("ESCODE_AGENT_SERVER_RUNTIME")) === "escode-cli-rust";
    if (rust) {
      // 必须在首次打开知识库页面前预置：runtime 的 Wiki 快照有 60s TTL。
      seededFile = await seedGlobalProposal();
    }

    await skipOccupationOnboardingIfPresent();
    await waitForDefaultWorkspaceReady(30000);
    await waitForWorkspaceApp(DEFAULT_WORKSPACE, 30000);
    await openKnowledgeSettings();

    if (!rust) {
      const notice = await $('[data-testid="knowledge-phase-notice"][data-phase="unsupported"]');
      await notice.waitForDisplayed({
        timeout: 30000,
        timeoutMsg: "Node runtime 下知识库页没有显示「不支持」提示",
      });
      expect(await $(sel("knowledge-graph-canvas")).isExisting()).toBe(false);
      return;
    }

    const node = await $(`[data-testid="knowledge-graph-node"][data-node-status="proposal"]`);
    await node.waitForDisplayed({ timeout: 30000, timeoutMsg: "图谱中没有出现预置的候选知识节点" });
    await node.click();
    const detail = await $(sel("knowledge-entry-detail"));
    await detail.waitForDisplayed({ timeout: 15000 });
    await browser.waitUntil(async () => (await detail.getText()).includes(ENTRY_TITLE), {
      timeout: 15000,
      timeoutMsg: "条目详情没有显示预置条目",
    });

    await clickTestIdByDom("knowledge-entry-approve", {
      timeout: 15000,
      timeoutMsg: "缺少批准按钮",
    });
    await $(sel("knowledge-review-confirm")).waitForDisplayed({
      timeout: 15000,
      timeoutMsg: "批准前没有弹出确认框",
    });
    await clickTestIdByDom(TID_CONFIRM_DIALOG_CONFIRM, {
      timeout: 15000,
      timeoutMsg: "确认框缺少确认按钮",
    });

    // 以 runtime 落盘为准：文件状态变为 approved，并带审核记录。
    await browser.waitUntil(
      async () => {
        const text = await readFile(seededFile!, "utf-8");
        return /^status: approved$/m.test(text) && /action: approve/.test(text);
      },
      { timeout: 20000, timeoutMsg: "批准后条目文件没有落盘为 approved" },
    );
    // 界面重拉后，批准按钮消失、出现弃用按钮。
    await $(sel("knowledge-entry-deprecate")).waitForDisplayed({
      timeout: 15000,
      timeoutMsg: "批准后详情没有刷新为已批准状态",
    });
    expect(await $(sel("knowledge-entry-approve")).isExisting()).toBe(false);
  });

  it("KG-02: 从「导入文档」导入到项目 Wiki，并把条目提升到全局", async function () {
    this.timeout(120000);
    if ((await appEnv("ESCODE_AGENT_SERVER_RUNTIME")) !== "escode-cli-rust") {
      // Node runtime 没有导入能力（KG-01 已验证不支持态）。
      this.skip();
    }
    const docDir = join(DEFAULT_WORKSPACE, "docs");
    await mkdir(docDir, { recursive: true });
    await writeFile(
      join(docDir, "kg-import.md"),
      "# KG import\n\n## Paging contract\n\nList APIs must return a next_cursor field.\n",
      "utf-8",
    );
    await ensureKnowledgeSettings();
    await clickTestIdByDom("knowledge-import-open", {
      timeout: 15000,
      timeoutMsg: "缺少「导入文档」入口",
    });
    await $(sel("knowledge-import-dialog")).waitForDisplayed({ timeout: 15000 });
    await $(sel("knowledge-import-paths")).setValue("docs/kg-import.md");
    // 默认目标为项目 Wiki。
    expect(await $(sel("knowledge-import-scope-project")).getAttribute("aria-checked")).toBe(
      "true",
    );
    await clickTestIdByDom("knowledge-import-submit", {
      timeout: 15000,
      timeoutMsg: "缺少导入按钮",
    });
    await $('[data-testid="knowledge-import-result"] li[data-status="imported"]').waitForDisplayed({
      timeout: 30000,
      timeoutMsg: "导入结果中没有 imported 行",
    });
    await clickTestIdByDom("knowledge-import-done", { timeout: 15000, timeoutMsg: "缺少完成按钮" });

    // 项目 Wiki 落在工作区内，只有知识本体入库。
    const projectWiki = join(DEFAULT_WORKSPACE, ".escode", "wiki");
    const entries = (await readdir(join(projectWiki, "entries"))).filter((f) => f.endsWith(".md"));
    expect(entries.length).toBeGreaterThan(0);
    expect(await readFile(join(projectWiki, ".gitignore"), "utf-8")).toContain("sources/");

    // 选中新条目 → 提升到全局 → 确认 → 全局存储出现 <id>.global.md。
    const node = await $(
      '[data-testid="knowledge-graph-node"][data-node-type="entry"]*=Paging contract',
    );
    await node.waitForDisplayed({ timeout: 30000, timeoutMsg: "图谱中没有出现导入的条目" });
    await node.click();
    await clickTestIdByDom("knowledge-entry-promote", {
      timeout: 15000,
      timeoutMsg: "缺少「提升到全局」",
    });
    await $(sel("knowledge-review-confirm")).waitForDisplayed({ timeout: 15000 });
    await clickTestIdByDom(TID_CONFIRM_DIALOG_CONFIRM, {
      timeout: 15000,
      timeoutMsg: "确认框缺少确认按钮",
    });
    const globalEntries = await wikiGlobalEntriesDir();
    await browser.waitUntil(
      async () =>
        (await readdir(globalEntries).catch(() => [])).some((f) => f.endsWith(".global.md")),
      { timeout: 20000, timeoutMsg: "提升后全局存储没有出现 .global 副本" },
    );
  });
});
