import { createHash } from "node:crypto";
import { cp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import {
  TID_PREVIEW_PANE,
  TID_V4_COMPOSER_INPUT,
  TID_WORKSPACE_FILE_TREE_BUTTON,
  TID_WORKSPACE_FILE_TREE_PANEL,
  TID_WORKSPACE_FILE_TREE_ROW,
  TID_WORKSPACE_ITEM,
  testId,
} from "@escode/shared";
import {
  DEFAULT_WORKSPACE,
  clearAppData,
  clickTestIdByDom,
  getE2EAppDataPaths,
  setInputValueByTestIdDom,
  waitForDefaultWorkspaceReady,
  waitForTestIdByDom,
  waitForWorkspaceApp,
} from "./helpers/desktop-app.js";
import { reloadElectronSessionSafely } from "./helpers/e2e-electron-reload.js";
import { seedHistoryFixtureIntoExistingDatabases } from "./helpers/provider-readiness-history-fixture.js";
import { selectV4TaskById, waitForV4TimelineContaining } from "./helpers/v4-conversation.js";

/**
 * 电路面板 Desktop E2E（docs/test-cases/e2e/circuit-panel.feature、规格 §10.3）。
 *
 * 覆盖：① 文件树打开 .kicad_sch → 原理图/BOM/网络表联动；② 面板改属性 → revision 加 1、作者=用户；
 * ③ 过期 baseRevision 写入被拒且不覆盖数据；④ 源文件外部变化 → stale → 重新解析按 UUID 保留修正；
 * ⑤ 对话里的 ::circuit 卡片 → 打开面板定位同一 IR（assistant 消息由 session 回放夹具注入，
 *    不调用真实模型；围栏内的同一行引用不渲染卡片）；
 * ⑥ 选中器件/网络插入到输入框 → 输入框上方出现引用 chip、草稿不发送。
 * ⑦（Web 降级卡片）在 Web/UI 层覆盖，见 packages/ui/test/circuit/circuitReferenceCardStatic.test.ts。
 * ⑧ 图片识别源：卡片 → 面板直读识别 IR，原图作底叠加识别框并联动 BOM / 网络表（IR 按服务的
 *    持久化布局预置，不调用真实模型）。
 *
 * 数据：packages/services/test/fixtures/circuit/rp2040-minimal（复制到临时 workspace，`.kicad_sch` /
 * `.kicad_pcb` / `.kicad_pro` 都带上）。每个场景用**独立的板子副本**，让每个场景都从 revision r1 开始
 * （IR 持久化按 identityKey + 源文件路径隔离，服务在窗口 Host 里按 workspace 作用域持有）。
 */

const FIXTURE_DIR = resolve(
  import.meta.dirname,
  "../../../../packages/services/test/fixtures/circuit/rp2040-minimal",
);
const HARDWARE_DIR = join(DEFAULT_WORKSPACE, "hardware");
const BOARD_FILE = "RP2040_minimal.kicad_sch";

/** fixture 里的稳定 UUID（KiCad 写出，IR 的器件 id 与原理图 symbol 的 elementId 都是它）。 */
const U1_ID = "00000000-0000-0000-0000-00005f04c8b7";
const R1_ID = "00000000-0000-0000-0000-00005edac067";
const R2_ID = "00000000-0000-0000-0000-00005edae9f0";

/**
 * 选中后的 SVG 高亮数量（由 rp2040-minimal 的解析结果决定：U1 是 LDO，接 GND / +3V3 / VBUS）。
 * 高亮规则见 packages/ui/src/circuit/domain/circuitView.ts：器件选中 → 它所有引脚所在的整条网络。
 */
const U1_HIGHLIGHT = { pins: 89, symbols: 25, wires: 89 };
const NET_3V3_HIGHLIGHT = { pins: 24, symbols: 16, wires: 24 };

/** 用户/Agent 作者标签（locale 相关：zh-CN 用户 / en-US User；Agent 两种 locale 都含 "Agent"）。 */
const USER_AUTHOR_LABEL = /(用户|User)/u;

interface CircuitPanelSnapshot {
  irId: string | null;
  readOnly: string | null;
  requestedIrId: string | null;
  revision: number;
  stale: string | null;
}

type CircuitSvgHighlight = { pins: number; symbols: number; wires: number } | null;

const CASE_NAMES = [
  "case-linkage",
  "case-corrections",
  "case-conflict",
  "case-reparse",
  "case-draft",
  "case-card",
] as const;

/** 场景 ⑤ 的 session 回放夹具（不调用真实模型，由 tasks-index + CLI session 冷恢复提供）。 */
const CARD_SESSION_ID = "sess_e2e_circuit_reference_card";
const CARD_TASK_TITLE = "E2E_CIRCUIT_REFERENCE_CARD 电路引用卡片";
const CARD_MARKER = "E2E_CIRCUIT_CARD_MARKER";

describe("电路面板 E2E", () => {
  before(async function () {
    this.timeout(180000);
    // 这台开发机的出口网络能打到真实账号接口，E2E 的 mock token 会被判 401；产品在 401 时走
    // 「登录失效 → 重启客户端」（app.relaunch + app.quit），会直接杀掉 E2E 窗口。这里只截住
    // 「重启 / 退出」两个平台动作，避免环境副作用中断用例；电路面板的行为断言不受影响。
    await browser.electron.mock("app", "relaunch");
    await browser.electron.mock("app", "quit");
    await waitForDefaultWorkspaceReady();
    // 一次性把 5 个场景的板子副本都放好：文件树的搜索索引在打开时建立，之后再新增目录不会被搜到。
    for (const caseName of CASE_NAMES) {
      const caseDir = join(HARDWARE_DIR, caseName);
      await rm(caseDir, { recursive: true, force: true });
      await mkdir(caseDir, { recursive: true });
      // `.kicad_sch` / `.kicad_pcb` / `.kicad_pro` 一起带上，与规格 §10.3 的测试数据一致。
      await cp(FIXTURE_DIR, caseDir, { recursive: true });
    }
  });

  after(async () => {
    await browser.electron.restoreAllMocks();
    await clearAppData();
    await rm(HARDWARE_DIR, { recursive: true, force: true });
  });

  it("在文件树中打开 .kicad_sch 后面板渲染图纸、BOM 与选中的网络高亮联动", async function () {
    this.timeout(180000);
    await openCircuitViewer(boardPath("case-linkage"));

    // 图纸与 r1：首次打开直接走确定性解析（§8「还没有 IR 时」），没有 stale 提示。
    await waitForCircuitPanelReady();
    expect(await readPanelSnapshot()).toMatchObject({
      revision: 1,
      stale: "false",
      requestedIrId: null,
    });
    expect(await readTestIdText("circuit-revision")).toBe("r1");
    expect(await hasTestId("circuit-stale-banner")).toBe(false);

    // BOM 列出解析出的器件（rp2040-minimal 解析出 34 个器件 / 52 条网络）。
    expect(await countElementsWithTestId("circuit-bom-row")).toBe(34);
    await clickTestIdByDom("circuit-side-tab-nets", { timeoutMsg: "网络表页签不可点击" });
    expect(await countElementsWithTestId("circuit-net-row")).toBe(52);
    await clickTestIdByDom("circuit-side-tab-bom", { timeoutMsg: "BOM 页签不可点击" });

    // 点击原理图里的 U1：器件高亮，连接 +3V3 的引脚与导线一并高亮，BOM 里 U1 同步选中。
    await clickCircuitSymbol(U1_ID);
    await expectCircuitSvgHighlight(U1_HIGHLIGHT, "选中 U1 后高亮范围与解析结果不一致");
    expect(
      await readElementAttribute(
        "circuit-symbol",
        "data-circuit-element-id",
        U1_ID,
        "data-circuit-selected",
      ),
    ).toBe("true");
    expect(await readRowAttribute("circuit-bom-row", U1_ID, "aria-pressed")).toBe("true");
    expect(await readTestIdText("circuit-selection")).toBe("U1");

    // 再点网络表里的 +3V3：网络一行的选中态与高亮范围切换到该条网络。
    await clickTestIdByDom("circuit-side-tab-nets", { timeoutMsg: "网络表页签不可点击" });
    await clickCircuitNetRow("+3V3");
    await expectCircuitSvgHighlight(NET_3V3_HIGHLIGHT, "选中 +3V3 后高亮范围与解析结果不一致");
    expect(await readRowAttributeWithTestId("circuit-net-row", "net:+3V3", "aria-pressed")).toBe(
      "true",
    );
    expect(await readTestIdText("circuit-selection")).toBe("+3V3");
  });

  it("面板内修改器件属性后 revision 加 1、修正记录作者是用户", async function () {
    this.timeout(180000);
    await openCircuitViewer(boardPath("case-corrections"));
    await waitForCircuitPanelReady();
    expect(await readTestIdText("circuit-revision")).toBe("r1");

    // 在 BOM 里选中 R1（值 DNF）→ 面板展示修正表单。
    await clickCircuitBomRow(R1_ID);
    await waitForTestIdByDom("circuit-edit-form", { timeoutMsg: "BOM 选中 R1 后没有出现修正表单" });
    expect(await readInputValue("circuit-edit-value")).toBe("DNF");
    expect(await readDisabled("circuit-edit-submit")).toBe(true);

    // 把值改成 4.7k 并应用修正：写入带 baseRevision，服务返回后 revision 加 1。
    await setInputValueByTestIdDom("circuit-edit-value", "4.7k", {
      timeoutMsg: "电路修正表单的值输入框不可输入",
    });
    await clickTestIdByDom("circuit-edit-submit", { timeoutMsg: "应用修正按钮不可点击" });
    await waitForRevision("r2", "应用修正后面板 revision 没有变成 r2");
    expect(await readCircuitBomRowText(R1_ID)).toContain("4.7k");

    // 修正记录：一条 r2 的「修改属性」记录，作者是用户（不是 Agent）。
    await clickTestIdByDom("circuit-side-tab-corrections", { timeoutMsg: "修正记录页签不可点击" });
    await waitForCorrectionRows(1, "修正记录没有出现刚写入的一条记录");
    const correctionText = await readTestIdText("circuit-correction-row");
    // 记录行 = 「修正内容 · 作者 · 时间 · 目标元素 id」；修正内容里的值来自 op，与 locale 无关。
    expect(correctionText).toContain("4.7k");
    expect(correctionText).toContain("r2");
    expect(correctionText).toContain(R1_ID);
    expect(USER_AUTHOR_LABEL.test(correctionText ?? "")).toBe(true);
    expect(correctionText).not.toContain("Agent");
    expect(await readTestIdAttribute("circuit-correction-row", "data-circuit-orphaned")).toBe(null);
  });

  it("过期 baseRevision 的写入被拒绝、不覆盖数据，刷新后显示外部写入后的值", async function () {
    this.timeout(180000);
    await openCircuitViewer(boardPath("case-conflict"));
    await waitForCircuitPanelReady();
    expect(await readTestIdText("circuit-revision")).toBe("r1");

    // 用户在面板里选中 R1 并把值改成 10k（先不提交）。
    await clickCircuitBomRow(R1_ID);
    await waitForTestIdByDom("circuit-edit-form", { timeoutMsg: "BOM 选中 R1 后没有出现修正表单" });
    await setInputValueByTestIdDom("circuit-edit-value", "10k", {
      timeoutMsg: "电路修正表单的值输入框不可输入",
    });

    // 同一 IR 被外部（Agent）先写成 r2，然后面板在同一个 tick 提交旧 baseRevision。
    // 说明：面板订阅 circuit.changed 会在事件落地后刷新；这里必须让外部写入与提交发生在
    // 事件被处理之前，才是「面板仍持 r1、服务已到 r2」的真实过期写入（见规格 §3.3）。
    const conflict = await writeFromAgentThenSubmitImmediately({
      componentId: R1_ID,
      externalValue: "1k",
      sessionId: "e2e-external-writer",
    });
    if (!conflict.ok) {
      throw new Error(`外部写入 + 面板提交的竞争构造失败: ${JSON.stringify(conflict)}`);
    }
    expect(conflict.localRevision).toBe(1);
    expect(conflict.externalRevision).toBe(2);

    // 面板提示写入被拒绝并建议刷新，revision 已经是外部的 r2。
    await waitForTestIdByDom("circuit-write-conflict", {
      timeout: 15000,
      timeoutMsg: "过期 baseRevision 的写入没有被拒绝，面板没有显示 stale_revision 提示",
    });
    await waitForRevision("r2", "外部写入后面板没有刷新到 r2");
    expect(await readTestIdText("circuit-revision")).toBe("r2");

    // 数据没有被 10k 覆盖：BOM 里 R1 显示外部写入的 1k（BOM 行文本 = 位号 · 值 + footprint + 引脚数）。
    const bomText = await readCircuitBomRowText(R1_ID);
    expect(bomText).toContain("R1 · 1k");
    expect(bomText).not.toContain("10k");

    // 点击提示里的刷新：冲突提示消失，仍然是外部写入后的数据。
    await clickTestIdByDom("circuit-write-conflict-refresh", {
      timeoutMsg: "写入冲突提示里的刷新按钮不可点击",
    });
    await waitUntilNoTestId("circuit-write-conflict", "刷新后写入冲突提示没有消失");
    expect(await readTestIdText("circuit-revision")).toBe("r2");
    expect(await readCircuitBomRowText(R1_ID)).toContain("R1 · 1k");
  });

  it("源文件外部变化后 stale 提示、重新解析按 UUID 保留修正并标记失效记录", async function () {
    this.timeout(240000);
    const boardDiskPath = boardPath("case-reparse");
    await openCircuitViewer(boardDiskPath);
    await waitForCircuitPanelReady();
    expect(await readTestIdText("circuit-revision")).toBe("r1");

    // 两条修正：R1 → 4.7k（r2），R2 → 2k（r3）。
    await applyCircuitCorrection(R1_ID, "4.7k", "r2");
    await applyCircuitCorrection(R2_ID, "2k", "r3");
    await clickTestIdByDom("circuit-side-tab-corrections", { timeoutMsg: "修正记录页签不可点击" });
    await waitForCorrectionRows(2, "两条修正没有都出现在修正记录里");

    // 外部把源文件里的 R2 删除并保存：watch 只翻转 stale，revision 不变。
    const source = await readFile(boardDiskPath, "utf-8");
    await writeFile(boardDiskPath, removeKicadSymbolBlock(source, R2_ID), "utf-8");
    await waitForTestIdByDom("circuit-stale-banner", {
      timeout: 20000,
      timeoutMsg: "外部修改源文件后面板没有出现 stale 提示",
    });
    expect(await readTestIdText("circuit-revision")).toBe("r3");

    // 重新解析：revision 加 1，R1（UUID 未变）的修正保留且未失效，R2 的修正标为已失效。
    await clickTestIdByDom("circuit-reparse", { timeoutMsg: "stale 提示里的重新解析按钮不可点击" });
    await waitUntilNoTestId("circuit-stale-banner", "重新解析后 stale 提示没有消失");
    await waitForRevision("r4", "重新解析后面板 revision 没有加 1");

    await clickTestIdByDom("circuit-side-tab-corrections", { timeoutMsg: "修正记录页签不可点击" });
    await waitForCorrectionRows(2, "重新解析后修正记录条数异常");
    const rows = await readCorrectionRows();
    const keptRow = rows.find((row) => row.text.includes("4.7k"));
    const orphanedRow = rows.find((row) => row.text.includes("2k"));
    expect(keptRow?.orphaned).toBe(null);
    expect(orphanedRow?.orphaned).toBe("true");
    expect(await countElementsWithTestId("circuit-correction-drop")).toBe(1);

    // 删除失效记录：只删掉那一条，其它修正不受影响。
    await clickTestIdByDom("circuit-correction-drop", { timeoutMsg: "失效记录的删除按钮不可点击" });
    await waitForCorrectionRows(1, "删除失效记录后列表没有回到一条");
    const remaining = await readCorrectionRows();
    expect(remaining[0]?.text).toContain("4.7k");
    expect(remaining[0]?.orphaned).toBe(null);
  });

  it("选中器件与网络插入输入框生成引用草稿，草稿没有自动发送", async function () {
    this.timeout(180000);
    await waitForTestIdByDom(TID_V4_COMPOSER_INPUT, {
      timeout: 20000,
      timeoutMsg: "工作区主界面没有 v4 输入框，无法验证引用草稿",
    });
    const messageRowsBefore = await countElementsWithTestIdPrefix("v4-row-");

    await openCircuitViewer(boardPath("case-draft"));
    const irId = (await readPanelSnapshot()).irId;
    expect(irId).not.toBe(null);
    await startCircuitDraftReferenceRecorder();

    // 选中 U1 → 插入到输入框：派发一条指向该器件的引用草稿，提示草稿未发送，并在输入框上方
    // 渲染出引用 chip（G5 曾漏掉 composer 只含电路引用时的渲染短路，见 ConversationComposer）。
    await clickCircuitBomRow(U1_ID);
    await clickTestIdByDom("circuit-insert-reference", { timeoutMsg: "插入到输入框按钮不可点击" });
    await waitForCircuitReferenceDrafts(1, "插入 U1 后没有生成电路引用草稿");
    // 草稿按 workspace 隔离：事件带的是文件树所在 workspace（本地 workspace 用路径作 key）。
    expect(await readCircuitDraftReferenceEvents()).toEqual([DEFAULT_WORKSPACE]);
    expect(await readToastText()).toContain("U1");
    expect(await readToastText()).toMatch(/(草稿未发送|not sent)/iu);
    await waitForComposerChipRow("插入 U1 后输入框上方没有出现引用 chip");
    expect(await readComposerChipCount()).toBe(1);
    expect(await readComposerChipHoverLabels()).toEqual(["U1"]);

    // 改选网络 +3V3 再插入：草稿累计到两条引用（网络引用带 NET: 前缀），chip 同步更新。
    await clickTestIdByDom("circuit-side-tab-nets", { timeoutMsg: "网络表页签不可点击" });
    await clickCircuitNetRow("+3V3");
    await clickTestIdByDom("circuit-insert-reference", { timeoutMsg: "插入到输入框按钮不可点击" });
    await waitForCircuitReferenceDrafts(2, "插入网络后草稿没有累计到两条引用");
    expect(await readToastText()).toContain("NET:+3V3");
    await waitForComposerChipCount(2, "插入网络后 chip 没有累计到两枚引用");
    expect(await readComposerChipHoverLabels()).toEqual(["U1", "NET:+3V3"]);

    // 草稿没有自动发送：对话行数不变、输入框仍然为空。
    expect(await countElementsWithTestIdPrefix("v4-row-")).toBe(messageRowsBefore);
    expect(await readComposerInputText()).toBe("");
  });

  it("点击对话里的 ::circuit 卡片打开面板并定位到同一 IR，围栏内的引用不渲染卡片", async function () {
    this.timeout(300000);
    // ① 先用文件树打开板子：拿到真实 irId（卡片消息必须引用已存在的 IR），面板落在 r1。
    await openCircuitViewer(boardPath("case-card"));
    await waitForCircuitPanelReady();
    const opened = await readPanelSnapshot();
    const irId = opened.irId;
    expect(irId).not.toBe(null);
    expect(opened.revision).toBe(1);

    // ② 固定 assistant 消息（session 回放夹具，不调用真实模型）：独占一行的 ::circuit 引用，
    //    外加 ```text 围栏里的同一行引用——围栏内不识别，正文保持原样。
    const referenceLine = `::circuit{"irId":"${irId}","title":"RP2040_minimal"}`;
    seedHistoryFixtureIntoExistingDatabases({
      e2eHomeDir: getE2EAppDataPaths().homeDir,
      history: {
        assistantInputTokens: 0,
        assistantOutputTokens: 0,
        assistantText: [
          CARD_MARKER,
          "",
          referenceLine,
          "",
          "```text",
          referenceLine,
          "```",
        ].join("\n"),
        modelId: "missing-model",
        runtimeProviderId: "missing-provider",
        sessionId: CARD_SESSION_ID,
        taskProviderId: "glm",
        title: CARD_TASK_TITLE,
        userText: `${CARD_MARKER}_USER 看一下这块板子的 I2C 连接`,
      },
      workspacePath: DEFAULT_WORKSPACE,
    });

    // ③ 整机冷启动：tasks-index membership 与 CLI session 正文只在 Host 冷启动时被读取（见
    //    provider-readiness-history-fixture）。冷启动后重新装回「登录失效不重启客户端」的两个 mock。
    await reloadElectronSessionSafely(browser);
    await browser.electron.mock("app", "relaunch");
    await browser.electron.mock("app", "quit");
    await waitForWorkspaceApp(DEFAULT_WORKSPACE, 60000);

    // ④ 从侧栏打开回放的会话：assistant 正文投影出 circuit-card（Desktop 形态）。
    await selectV4TaskById(CARD_SESSION_ID, 60000);
    await waitForV4TimelineContaining(CARD_MARKER, 60000);
    await waitForTestIdByDom("circuit-card", {
      timeout: 20000,
      timeoutMsg: "回放的 assistant 消息没有渲染 circuit-card",
    });
    expect(await readTestIdAttribute("circuit-card", "data-circuit-card-mode")).toBe("desktop");
    expect(await readTestIdText("circuit-card")).toContain("RP2040_minimal");
    // 围栏内的同一行引用不渲染卡片：整条消息只有一张卡片，代码块里的正文原样保留。
    expect(await countElementsWithTestId("circuit-card")).toBe(1);
    try {
      await waitForTimelineRawText(referenceLine, "围栏里的 ::circuit 引用没有原样保留在正文里");
    } catch (error) {
      throw new Error(
        `围栏里的 ::circuit 引用没有原样保留在正文里；诊断=${await readAssistantRowDiagnostics()}`,
        { cause: error },
      );
    }

    // ⑤ 点击「打开面板」：右侧打开电路面板并定位到卡片引用的 IR；IR 已存在，直接读取不变 revision。
    await clickTestIdByDom("circuit-card-open", { timeoutMsg: "引用卡片的打开面板按钮不可点击" });
    await waitForTestIdByDom(TID_PREVIEW_PANE, {
      timeout: 20000,
      timeoutMsg: "点击引用卡片后没有打开 PreviewPane",
    });
    await waitForCircuitPanelReady();
    const panel = await readPanelSnapshot();
    expect(panel.irId).toBe(irId);
    expect(panel.requestedIrId).toBe(irId);
    expect(panel.revision).toBe(1);
    expect(await readTestIdText("circuit-revision")).toBe("r1");
    expect(await hasTestId("circuit-stale-banner")).toBe(false);
  });

});

// ── workspace / 文件树 ────────────────────────────────────────────────────────

/** 每个场景一块独立副本，IR 按 identityKey + 源文件路径隔离，因此都从 revision r1 开始。 */
function boardPath(caseName: (typeof CASE_NAMES)[number]): string {
  return join(HARDWARE_DIR, caseName, BOARD_FILE);
}

/** 打开文件树 → 搜索板子所在目录 → 点击 .kicad_sch 行 → 等右侧 PreviewPane 渲染。 */
async function openCircuitViewer(boardPath: string) {
  await openWorkspaceFileTree();
  await typeWorkspaceFileTreeSearch(boardPath.split(/[\\/]/u).at(-2) ?? "");
  await waitForWorkspaceFileTreeRow(boardPath);
  await clickWorkspaceFileTreeRow(boardPath);
  await waitForTestIdByDom(TID_PREVIEW_PANE, {
    timeout: 20000,
    timeoutMsg: "文件树点击 .kicad_sch 后没有打开 PreviewPane",
  });
  await waitForCircuitPanelReady();
}

/**
 * 等面板完成首次解析（§8「还没有 IR 时直接解析」）。
 *
 * 实测（runner 日志）：打开同一个 .kicad_sch 时会有两路 initial resolve 并发进入 Host（窗口布局切换
 * 期间组件重建了两遍），先提交的那次写入 r1，后到的那次碰到「已有 IR」被 invalid_ir 拒绝，面板落到
 * 自带的「重试解析」状态。这里按产品提供的重试入口恢复；面板行为断言不做任何放宽。
 */
async function waitForCircuitPanelReady() {
  const deadline = Date.now() + 30000;
  let diagnostics = "页面里没有出现电路面板节点";
  while (Date.now() < deadline) {
    if (await hasTestId("circuit-schematic")) return;
    if (await hasTestId("circuit-panel-retry")) {
      diagnostics = await readCircuitPanelDiagnostics();
      await clickTestIdByDom("circuit-panel-retry", {
        timeoutMsg: "电路面板的重试解析按钮不可点击",
      });
      continue;
    }
    await browser.pause(250);
  }
  throw new Error(`电路面板没有进入 ready 状态；诊断: ${diagnostics}`);
}

/** 面板失败态的现场摘要（只在等待超时时使用，便于报告真实原因）。 */
async function readCircuitPanelDiagnostics(): Promise<string> {
  return (await browser.execute(() =>
    JSON.stringify({
      bodyText: document.body.innerText.replace(/\s+/gu, " ").slice(0, 300),
      panel: (() => {
        const panel = Array.from(document.querySelectorAll<HTMLElement>("[data-testid]")).find(
          (element) => element.dataset.testid === "circuit-panel",
        );
        return panel
          ? {
              revision: panel.getAttribute("data-circuit-revision"),
              stale: panel.getAttribute("data-circuit-stale"),
              irId: panel.getAttribute("data-circuit-ir-id"),
            }
          : null;
      })(),
      testIds: Array.from(document.querySelectorAll<HTMLElement>("[data-testid]"))
        .map((element) => element.dataset.testid ?? "")
        .filter((id) => id.startsWith("circuit"))
        .slice(0, 20),
    }),
  )) as string;
}

async function openWorkspaceFileTree() {
  const alreadyOpen = await hasTestId(TID_WORKSPACE_FILE_TREE_PANEL);
  if (alreadyOpen) {
    // 同一 spec 的后续场景复用已打开的文件树；workspace 行可能被面板挤出视口，不必重复 hover。
    return;
  }
  await revealWorkspaceRowActions(DEFAULT_WORKSPACE);
  await clickTestIdByDom(testId(TID_WORKSPACE_FILE_TREE_BUTTON, DEFAULT_WORKSPACE), {
    timeoutMsg: "没有找到工作区文件树入口",
  });
  await waitForTestIdByDom(TID_WORKSPACE_FILE_TREE_PANEL, { timeoutMsg: "文件树面板没有打开" });
}

/**
 * workspace 行的操作按钮只在 mouseenter / focus 之后挂载。
 * 这里不用 hoverTestIdByWebDriver：它的 `sel()` 会把 Windows 路径里的反斜杠拼进 CSS 属性
 * 选择器并被当成转义字符（匹配不到元素）；按 testid 精确找到行、派发冒泡的 mouseover 让
 * React 合成 onMouseEnter，再走 DOM 点击。
 */
async function revealWorkspaceRowActions(workspacePath: string) {
  const rowTestId = testId(TID_WORKSPACE_ITEM, workspacePath);
  await browser.waitUntil(
    async () =>
      browser.execute((currentRowTestId) => {
        const row = Array.from(document.querySelectorAll<HTMLElement>("[data-testid]")).find(
          (element) => element.dataset.testid === currentRowTestId,
        );
        if (!row) return false;
        row.dispatchEvent(new MouseEvent("mouseover", { bubbles: true, relatedTarget: null }));
        return true;
      }, rowTestId),
    { timeout: 20000, timeoutMsg: `侧栏没有 workspace 行: ${workspacePath}` },
  );
}

async function clickWorkspaceFileTreeRow(path: string) {
  const rowTestId = testId(TID_WORKSPACE_FILE_TREE_ROW, path);
  await browser.waitUntil(
    async () =>
      browser.execute((currentRowTestId) => {
        const row = Array.from(document.querySelectorAll<HTMLElement>("[data-testid]")).find(
          (element) => element.dataset.testid === currentRowTestId,
        );
        if (!row) return false;
        row.click();
        return true;
      }, rowTestId),
    { timeout: 15000, timeoutMsg: `文件树没有可点击行: ${path}` },
  );
}

async function waitForWorkspaceFileTreeRow(path: string) {
  const rowTestId = testId(TID_WORKSPACE_FILE_TREE_ROW, path);
  await browser.waitUntil(
    async () =>
      browser.execute(
        (currentRowTestId) =>
          Array.from(document.querySelectorAll<HTMLElement>("[data-testid]")).some(
            (element) => element.dataset.testid === currentRowTestId,
          ),
        rowTestId,
      ),
    { timeout: 15000, timeoutMsg: `文件树没有显示目标路径: ${path}` },
  );
}

async function typeWorkspaceFileTreeSearch(query: string) {
  await browser.waitUntil(
    async () =>
      browser.execute(
        (panelTestId, nextQuery) => {
          const panel = document.querySelector<HTMLElement>(`[data-testid="${panelTestId}"]`);
          const input = panel?.querySelector<HTMLInputElement>("input");
          if (!input) return false;
          const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set;
          setter?.call(input, nextQuery);
          input.dispatchEvent(new Event("input", { bubbles: true }));
          return true;
        },
        TID_WORKSPACE_FILE_TREE_PANEL,
        query,
      ),
    { timeout: 10000, timeoutMsg: "文件树搜索框不可输入" },
  );
}

// ── 面板 DOM 读取 / 操作 ──────────────────────────────────────────────────────

async function readPanelSnapshot(): Promise<CircuitPanelSnapshot> {
  return (await browser.execute(() => {
    const panel = Array.from(document.querySelectorAll<HTMLElement>("[data-testid]")).find(
      (element) => element.dataset.testid === "circuit-panel",
    );
    if (!panel) return null;
    return {
      irId: panel.getAttribute("data-circuit-ir-id"),
      readOnly: panel.getAttribute("data-circuit-read-only"),
      requestedIrId: panel.getAttribute("data-circuit-requested-ir-id"),
      revision: Number(panel.getAttribute("data-circuit-revision")),
      stale: panel.getAttribute("data-circuit-stale"),
    };
  })) as CircuitPanelSnapshot;
}

async function clickCircuitBomRow(componentId: string) {
  await clickElementByAttribute("circuit-bom-row", "data-circuit-element-id", componentId);
}

async function clickCircuitSymbol(componentId: string) {
  // SVG <g> 没有 HTMLElement.click()，用冒泡的 MouseEvent 让 React 的 onClick 收到。
  let presentIds: string[] = [];
  try {
    await browser.waitUntil(
      async () =>
        browser.execute(
          (testIdValue, attribute, value) => {
            const element = Array.from(document.querySelectorAll<SVGElement>("[data-testid]")).find(
              (candidate) =>
                candidate.dataset.testid === testIdValue &&
                candidate.getAttribute(attribute) === value,
            );
            if (!element) return false;
            element.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));
            return true;
          },
          "circuit-symbol",
          "data-circuit-element-id",
          componentId,
        ),
      { timeout: 15000, timeoutMsg: `原理图里找不到可点击的器件: ${componentId}` },
    );
  } catch (error) {
    presentIds = (await browser.execute(() =>
      Array.from(document.querySelectorAll<SVGElement>('[data-testid="circuit-symbol"]'))
        .map((element) => element.getAttribute("data-circuit-element-id") ?? "")
        .slice(0, 10),
    )) as string[];
    throw new Error(`${String(error)}
诊断: 原理图里的前 10 个器件 id=${JSON.stringify(presentIds)}`);
  }
}

async function clickCircuitNetRow(netName: string) {
  await browser.waitUntil(
    async () =>
      browser.execute((expectedName) => {
        const rows = Array.from(document.querySelectorAll<HTMLElement>("[data-testid]")).filter(
          (element) => element.dataset.testid === "circuit-net-row",
        );
        const row = rows.find((element) =>
          (element.textContent ?? "").trim().startsWith(expectedName),
        );
        if (!row) return false;
        row.click();
        return true;
      }, netName),
    { timeout: 15000, timeoutMsg: `网络表里找不到网络行: ${netName}` },
  );
}

async function clickElementByAttribute(testIdValue: string, attribute: string, value: string) {
  await browser.waitUntil(
    async () =>
      browser.execute(
        (currentTestId, currentAttribute, currentValue) => {
          const element = Array.from(document.querySelectorAll<HTMLElement>("[data-testid]")).find(
            (candidate) =>
              candidate.dataset.testid === currentTestId &&
              candidate.getAttribute(currentAttribute) === currentValue,
          );
          if (!element) return false;
          element.click();
          return true;
        },
        testIdValue,
        attribute,
        value,
      ),
    { timeout: 15000, timeoutMsg: `面板里找不到可点击元素: ${testIdValue}[${attribute}=${value}]` },
  );
}

async function readCircuitBomRowText(componentId: string): Promise<string> {
  const text = await browser.execute((currentComponentId) => {
    const row = Array.from(document.querySelectorAll<HTMLElement>("[data-testid]")).find(
      (element) =>
        element.dataset.testid === "circuit-bom-row" &&
        element.getAttribute("data-circuit-element-id") === currentComponentId,
    );
    return row?.textContent ?? null;
  }, componentId);
  expect(text).not.toBe(null);
  return (text ?? "").trim();
}

async function readCorrectionRows(): Promise<{ orphaned: string | null; text: string }[]> {
  return (await browser.execute(() =>
    Array.from(document.querySelectorAll<HTMLElement>("[data-testid]"))
      .filter((element) => element.dataset.testid === "circuit-correction-row")
      .map((element) => ({
        orphaned: element.getAttribute("data-circuit-orphaned"),
        text: (element.textContent ?? "").trim(),
      })),
  )) as { orphaned: string | null; text: string }[];
}

async function waitForCorrectionRows(count: number, timeoutMsg: string) {
  await browser.waitUntil(
    async () => (await countElementsWithTestId("circuit-correction-row")) === count,
    { timeout: 15000, timeoutMsg },
  );
}

/** SVG 里的高亮数量：polyline=导线、line=引脚、带 data-circuit-element-id 的 g=器件。 */
async function readCircuitSvgHighlight(): Promise<CircuitSvgHighlight> {
  return (await browser.execute(() => {
    const svg = Array.from(document.querySelectorAll<SVGElement>("[data-testid]")).find(
      (element) => element.dataset.testid === "circuit-schematic",
    );
    if (!svg) return null;
    return {
      pins: svg.querySelectorAll("line.stroke-brand").length,
      symbols: svg.querySelectorAll('g[data-circuit-element-id][class*="text-brand"]').length,
      wires: svg.querySelectorAll("polyline.stroke-brand").length,
    };
  })) as CircuitSvgHighlight;
}

async function expectCircuitSvgHighlight(
  expected: { pins: number; symbols: number; wires: number },
  timeoutMsg: string,
) {
  await browser.waitUntil(
    async () => {
      const current = await readCircuitSvgHighlight();
      return (
        current !== null &&
        current.pins === expected.pins &&
        current.wires === expected.wires &&
        current.symbols === expected.symbols
      );
    },
    { timeout: 15000, timeoutMsg },
  );
  expect(await readCircuitSvgHighlight()).toEqual(expected);
}

/** 输入框上方出现引用 chip：chip 必须落在 composer 的上下文附件行里，且整行位于输入框上方。 */
async function waitForComposerChipRow(timeoutMsg: string) {
  await browser.waitUntil(
    async () => (await countElementsWithTestId("circuit-reference-chip")) === 1,
    { timeout: 15000, timeoutMsg },
  );
  const placement = (await browser.execute((inputTestId) => {
    const chip = document.querySelector<HTMLElement>('[data-testid="circuit-reference-chip"]');
    const row = chip?.closest<HTMLElement>("[data-composer-context-attachments-row]") ?? null;
    const input = Array.from(document.querySelectorAll<HTMLElement>("[data-testid]")).find(
      (element) => element.dataset.testid === inputTestId,
    );
    if (!chip || !row || !input) return null;
    return {
      inComposerForm: Boolean(input.closest("form")?.contains(row)),
      inputTop: input.getBoundingClientRect().top,
      rowBottom: row.getBoundingClientRect().bottom,
      rowCount: document.querySelectorAll("[data-composer-context-attachments-row]").length,
    };
  }, TID_V4_COMPOSER_INPUT)) as {
    inComposerForm: boolean;
    inputTop: number;
    rowBottom: number;
    rowCount: number;
  } | null;
  expect(placement).not.toBe(null);
  // 只有 composer 的一行上下文附件；chip 与输入框同在一个 form 内，且整行在输入框上方。
  expect(placement?.rowCount).toBe(1);
  expect(placement?.inComposerForm).toBe(true);
  expect(placement?.rowBottom ?? Number.POSITIVE_INFINITY).toBeLessThanOrEqual(
    (placement?.inputTop ?? 0) + 1,
  );
}

async function waitForComposerChipCount(count: number, timeoutMsg: string) {
  await browser.waitUntil(
    async () => (await readComposerChipCount()) === count,
    { timeout: 15000, timeoutMsg },
  );
}

async function readComposerChipCount(): Promise<number | null> {
  const raw = await browser.execute(() => {
    const chip = document.querySelector<HTMLElement>('[data-testid="circuit-reference-chip"]');
    return chip?.getAttribute("data-circuit-reference-count") ?? null;
  });
  return raw === null ? null : Number(raw);
}

/** chip 详情浮层里的逐条引用（Radix HoverCard 内容走 Portal，聚焦 trigger 即展开）。 */
async function readComposerChipHoverLabels(): Promise<string[]> {
  await browser.execute(() => {
    const chip = document.querySelector<HTMLElement>('[data-testid="circuit-reference-chip"]');
    chip?.dispatchEvent(new FocusEvent("focusin", { bubbles: true, relatedTarget: null }));
  });
  await browser.waitUntil(
    async () => (await countElementsWithTestId("circuit-reference-chip-label")) > 0,
    { timeout: 5000, timeoutMsg: "聚焦引用 chip 后没有展开详情浮层" },
  );
  const labels = (await browser.execute(() =>
    Array.from(document.querySelectorAll<HTMLElement>('[data-testid="circuit-reference-chip-label"]'))
      .map((element) => (element.textContent ?? "").trim()),
  )) as string[];
  await browser.execute(() => {
    const chip = document.querySelector<HTMLElement>('[data-testid="circuit-reference-chip"]');
    chip?.dispatchEvent(new FocusEvent("focusout", { bubbles: true, relatedTarget: null }));
  });
  return labels;
}

/** 面板「插入到输入框」派发的 window 事件（renderer 草稿，不发送）。 */
const CIRCUIT_DRAFT_REFERENCE_EVENT = "escode:circuit-reference-draft-add";

async function startCircuitDraftReferenceRecorder() {
  await browser.execute((eventName) => {
    const target = window as Window & { __circuitDraftProbe?: { events: string[] } };
    target.__circuitDraftProbe = { events: [] };
    window.addEventListener(eventName, (event) => {
      const detail = (event as CustomEvent<{ workspaceKey?: string }>).detail;
      target.__circuitDraftProbe?.events.push(detail?.workspaceKey ?? "");
    });
  }, CIRCUIT_DRAFT_REFERENCE_EVENT);
}

async function readCircuitDraftReferenceEvents(): Promise<string[]> {
  return (await browser.execute(
    () =>
      (window as Window & { __circuitDraftProbe?: { events: string[] } }).__circuitDraftProbe
        ?.events ?? [],
  )) as string[];
}

async function waitForCircuitReferenceDrafts(count: number, timeoutMsg: string) {
  await browser.waitUntil(async () => (await readCircuitDraftReferenceEvents()).length === count, {
    timeout: 15000,
    timeoutMsg,
  });
}

/** 面板插入引用后的用户可见提示（toast host，文案由产品 i18n 提供）。 */
async function readToastText(): Promise<string> {
  return (await browser.execute(() => {
    const host = document.querySelector<HTMLElement>("#escode-toast-host");
    return host?.innerText?.replace(/\s+/gu, " ") ?? "";
  })) as string;
}

async function readComposerInputText(): Promise<string | null> {
  return (await browser.execute((testIdValue) => {
    const input = Array.from(document.querySelectorAll<HTMLElement>("[data-testid]")).find(
      (element) => element.dataset.testid === testIdValue,
    ) as (HTMLElement & { __escodeLexicalInputE2E?: { getText: () => string } }) | undefined;
    if (!input) return null;
    // v4 composer 是 Lexical contenteditable：文本以编辑器 state 为准（DOM 直读会漏恢复中的草稿）。
    return (input.__escodeLexicalInputE2E?.getText() ?? input.textContent ?? "").trim();
  }, TID_V4_COMPOSER_INPUT)) as string | null;
}

// ── 通用断言 / 等待 ──────────────────────────────────────────────────────────

async function hasTestId(testIdValue: string): Promise<boolean> {
  return (await browser.execute((currentTestId) => {
    const element = Array.from(document.querySelectorAll<HTMLElement>("[data-testid]")).find(
      (candidate) => candidate.dataset.testid === currentTestId,
    );
    return Boolean(element);
  }, testIdValue)) as boolean;
}

async function waitUntilNoTestId(testIdValue: string, timeoutMsg: string) {
  await browser.waitUntil(async () => !(await hasTestId(testIdValue)), {
    timeout: 15000,
    timeoutMsg,
  });
}

async function readTestIdText(testIdValue: string): Promise<string | null> {
  return (await browser.execute((currentTestId) => {
    const element = Array.from(document.querySelectorAll<HTMLElement>("[data-testid]")).find(
      (candidate) => candidate.dataset.testid === currentTestId,
    );
    return element?.textContent?.trim() ?? null;
  }, testIdValue)) as string | null;
}

/**
 * 围栏里的 directive 必须原样出现在对话流里（正文保持原样、没有被投影成卡片）。
 * 代码块（@pierre/diffs）的行文本在异步创建的 Shadow DOM 中：先滚动 assistant 行让它渲染，
 * 再同时收集 light DOM 与 shadow root 的文本节点。
 */
async function waitForTimelineRawText(text: string, timeoutMsg: string) {
  await browser.waitUntil(
    async () =>
      browser.execute((needle) => {
        const collectText = (root: Node): string => {
          let collected = "";
          const walk = (node: Node) => {
            if (node.nodeType === Node.TEXT_NODE) {
              collected += node.nodeValue ?? "";
              return;
            }
            // ShadowRoot 是 DOCUMENT_FRAGMENT_NODE：不认这一类会漏掉 @pierre/diffs 的代码行。
            const isTraversable =
              node.nodeType === Node.ELEMENT_NODE ||
              node.nodeType === Node.DOCUMENT_NODE ||
              node.nodeType === Node.DOCUMENT_FRAGMENT_NODE;
            if (!isTraversable) return;
            const shadow = (node as Element).shadowRoot;
            if (shadow) walk(shadow);
            for (const child of Array.from(node.childNodes)) walk(child);
          };
          walk(root);
          return collected;
        };
        const rows = Array.from(
          document.querySelectorAll<HTMLElement>('[data-conversation-selectable="true"]'),
        );
        for (const row of rows) row.scrollIntoView({ block: "center" });
        return rows.some((row) => collectText(row).includes(needle));
      }, text),
    { timeout: 20000, timeoutMsg },
  );
}

/** 断言失败时的现场摘要：assistant 行的 HTML 片段（含内容长度）。 */
async function readAssistantRowDiagnostics(): Promise<string> {
  return (await browser.execute(() =>
    JSON.stringify(
      Array.from(document.querySelectorAll<HTMLElement>('[data-conversation-selectable="true"]')).map(
        (row) => ({
          html: (row.innerHTML ?? "").replace(/\s+/gu, " ").slice(0, 600),
          length: (row.textContent ?? "").length,
        }),
      ),
    ),
  )) as string;
}

async function readTestIdAttribute(testIdValue: string, attribute: string): Promise<string | null> {
  return (await browser.execute(
    (currentTestId, currentAttribute) => {
      const element = Array.from(document.querySelectorAll<HTMLElement>("[data-testid]")).find(
        (candidate) => candidate.dataset.testid === currentTestId,
      );
      return element?.getAttribute(currentAttribute) ?? null;
    },
    testIdValue,
    attribute,
  )) as string | null;
}

async function readElementAttribute(
  testIdValue: string,
  keyAttribute: string,
  keyValue: string,
  readAttribute: string,
): Promise<string | null> {
  return (await browser.execute(
    (currentTestId, currentKeyAttribute, currentKeyValue, currentReadAttribute) => {
      const element = Array.from(document.querySelectorAll<HTMLElement>("[data-testid]")).find(
        (candidate) =>
          candidate.dataset.testid === currentTestId &&
          candidate.getAttribute(currentKeyAttribute) === currentKeyValue,
      );
      return element?.getAttribute(currentReadAttribute) ?? null;
    },
    testIdValue,
    keyAttribute,
    keyValue,
    readAttribute,
  )) as string | null;
}

async function readRowAttribute(
  testIdValue: string,
  elementId: string,
  readAttribute: string,
): Promise<string | null> {
  return readRowAttributeWithTestId(testIdValue, elementId, readAttribute);
}

async function readRowAttributeWithTestId(
  testIdValue: string,
  elementId: string,
  readAttribute: string,
  keyAttribute = "data-circuit-element-id",
): Promise<string | null> {
  return readElementAttribute(testIdValue, keyAttribute, elementId, readAttribute);
}

async function readInputValue(testIdValue: string): Promise<string | null> {
  return (await browser.execute((currentTestId) => {
    const element = Array.from(document.querySelectorAll<HTMLInputElement>("[data-testid]")).find(
      (candidate) => candidate.dataset.testid === currentTestId,
    );
    return element?.value ?? null;
  }, testIdValue)) as string | null;
}

async function readDisabled(testIdValue: string): Promise<boolean | null> {
  return (await browser.execute((currentTestId) => {
    const element = Array.from(
      document.querySelectorAll<HTMLButtonElement | HTMLInputElement>("[data-testid]"),
    ).find((candidate) => candidate.dataset.testid === currentTestId);
    return element ? element.disabled : null;
  }, testIdValue)) as boolean | null;
}

async function countElementsWithTestId(testIdValue: string): Promise<number> {
  return (await browser.execute((currentTestId) => {
    return Array.from(document.querySelectorAll<HTMLElement>("[data-testid]")).filter(
      (candidate) => candidate.dataset.testid === currentTestId,
    ).length;
  }, testIdValue)) as number;
}

async function countElementsWithTestIdPrefix(prefix: string): Promise<number> {
  return (await browser.execute((currentPrefix) => {
    return Array.from(document.querySelectorAll<HTMLElement>("[data-testid]")).filter((candidate) =>
      (candidate.dataset.testid ?? "").startsWith(currentPrefix),
    ).length;
  }, prefix)) as number;
}

async function waitForRevision(expected: string, timeoutMsg: string) {
  await browser.waitUntil(async () => (await readTestIdText("circuit-revision")) === expected, {
    timeout: 20000,
    timeoutMsg,
  });
}

/** 面板里改一个器件属性并等 revision 前进（BOM 选中 → 填值 → 应用修正）。 */
async function applyCircuitCorrection(
  componentId: string,
  value: string,
  expectedRevision: string,
) {
  await clickTestIdByDom("circuit-side-tab-bom", { timeoutMsg: "BOM 页签不可点击" });
  await clickCircuitBomRow(componentId);
  await waitForTestIdByDom("circuit-edit-form", {
    timeoutMsg: `BOM 选中 ${componentId} 后没有出现修正表单`,
  });
  await setInputValueByTestIdDom("circuit-edit-value", value, {
    timeoutMsg: "电路修正表单的值输入框不可输入",
  });
  await clickTestIdByDom("circuit-edit-submit", { timeoutMsg: "应用修正按钮不可点击" });
  await waitForRevision(expectedRevision, `应用修正后面板 revision 没有变成 ${expectedRevision}`);
}

/**
 * 「外部写入 + 面板提交」的竞争构造（只用于过期 baseRevision 场景）。
 * 面板的写路径必须经过它自己的 hook（这里用 React fiber 上的 props.services 拿同一个 workspace
 * 服务代理），才能复用真实 CircuitService：写入按 irId 串行、baseRevision 落后即拒绝。
 */
async function writeFromAgentThenSubmitImmediately(params: {
  componentId: string;
  externalValue: string;
  sessionId: string;
}): Promise<{
  ok: boolean;
  reason?: string;
  externalRevision?: number | null;
  localRevision?: number;
}> {
  return (await browser.executeAsync((payload, done) => {
    type FiberLike = {
      memoizedProps?: Record<string, unknown> | null;
      pendingProps?: Record<string, unknown> | null;
      return: FiberLike | null;
    };
    const findById = (id: string) =>
      Array.from(document.querySelectorAll<HTMLElement>("[data-testid]")).find(
        (element) => element.dataset.testid === id,
      );
    const panel = findById("circuit-panel");
    if (!panel) return done({ ok: false, reason: "circuit-panel-missing" });
    const fiberKey = Object.keys(panel).find((key) => key.startsWith("__reactFiber$"));
    let fiber = fiberKey
      ? ((panel as unknown as Record<string, FiberLike>)[fiberKey] ?? null)
      : null;
    let panelProps: Record<string, unknown> | null = null;
    while (fiber && !panelProps) {
      const candidate = fiber.memoizedProps ?? fiber.pendingProps;
      if (candidate && candidate.services && candidate.workspace) panelProps = candidate;
      fiber = fiber.return;
    }
    const services = panelProps?.services as
      | {
          circuitService?: {
            correct: (params: unknown) => Promise<{ summary: { revision: number } }>;
          };
        }
      | undefined;
    const workspace = panelProps?.workspace as
      | { workspacePath?: string; workspaceIdentity?: string; remoteSessionId?: string }
      | undefined;
    const circuitService = services?.circuitService;
    if (!circuitService || !workspace?.workspacePath) {
      return done({ ok: false, reason: "circuit-service-unavailable" });
    }
    const irId = panel.getAttribute("data-circuit-ir-id");
    const baseRevision = Number(panel.getAttribute("data-circuit-revision"));
    circuitService
      .correct({
        workspacePath: workspace.workspacePath,
        ...(workspace.workspaceIdentity ? { workspaceIdentity: workspace.workspaceIdentity } : {}),
        ...(workspace.remoteSessionId ? { remoteSessionId: workspace.remoteSessionId } : {}),
        irId,
        baseRevision,
        ops: [
          {
            op: "update-component",
            componentId: payload.componentId,
            value: payload.externalValue,
          },
        ],
        author: { kind: "agent", sessionId: payload.sessionId },
      })
      .then((view) => {
        // 同一个 tick 里立刻点「应用修正」：此时 circuit.changed 还没被 renderer 处理，
        // 面板仍持旧 revision，服务已推进到外部写入后的 revision —— 真实过期写入。
        const submit = findById("circuit-edit-submit");
        if (!submit) {
          return done({
            ok: false,
            reason: "circuit-edit-submit-missing",
            externalRevision: view?.summary?.revision ?? null,
          });
        }
        submit.click();
        done({
          ok: true,
          externalRevision: view?.summary?.revision ?? null,
          localRevision: baseRevision,
        });
      })
      .catch((error: unknown) => {
        done({ ok: false, reason: error instanceof Error ? error.message : String(error) });
      });
  }, params)) as {
    ok: boolean;
    reason?: string;
    externalRevision?: number | null;
    localRevision?: number;
  };
}

/** 外部改源文件：按 UUID 定位 `(symbol …)` 块并整块删除（字符串里的括号不参与配对）。 */
function removeKicadSymbolBlock(source: string, uuid: string): string {
  const uuidIndex = source.indexOf(uuid);
  if (uuidIndex < 0) throw new Error(`源文件里找不到 UUID: ${uuid}`);
  const start = source.lastIndexOf("(symbol", uuidIndex);
  if (start < 0) throw new Error(`UUID 之前找不到 symbol 块: ${uuid}`);
  let depth = 0;
  let inString = false;
  for (let index = start; index < source.length; index += 1) {
    const char = source[index]!;
    if (inString) {
      if (char === "\\") index += 1;
      else if (char === '"') inString = false;
      continue;
    }
    if (char === '"') inString = true;
    else if (char === "(") depth += 1;
    else if (char === ")") {
      depth -= 1;
      if (depth === 0) return source.slice(0, start) + source.slice(index + 1);
    }
  }
  throw new Error(`symbol 块没有闭合: ${uuid}`);
}
