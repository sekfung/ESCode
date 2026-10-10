import assert from "node:assert/strict";
import test from "node:test";
import type { SerialChunk, SerialSnapshot } from "@zcode/services";
import {
  buildSerialSendPayload,
  createSerialStreamDecoder,
  formatSerialHex,
  parseSerialHexInput,
} from "../src/lib/serial/serialFormat.js";
import {
  applySerialChunk,
  applySerialSnapshot,
  createSerialChunkLog,
} from "../src/lib/serial/serialChunkLog.js";
import {
  getVisibleSidePaneTabs,
  openSerialSidePane,
  stampSidePaneTabsOwnership,
} from "../src/lib/workspaceSidePane.js";
import { resolveOpenTabLauncherItemIds } from "../src/app-shell/animatedSidePanePanelModel.js";

const bytes = (...values: number[]) => Uint8Array.from(values);
const text = (value: string) => new TextEncoder().encode(value);

function chunk(seq: number, direction: SerialChunk["direction"] = "rx"): SerialChunk {
  return { seq, at: 1_000 + seq, direction, source: "user", bytes: bytes(seq) };
}

function snapshot(chunks: SerialChunk[], seq = chunks.at(-1)?.seq ?? 0): SerialSnapshot {
  return {
    status: { state: "open", path: "COM3" },
    chunks,
    seq,
    stats: { rxBytes: 0, txBytes: 0 },
  };
}

// --- HEX / 文本编解码 ---------------------------------------------------------

test("HEX 输入支持空格、逗号、换行分隔，大小写与 0x 前缀", () => {
  assert.deepEqual(parseSerialHexInput("41 54\n0d,0A 0xff"), {
    ok: true,
    bytes: bytes(0x41, 0x54, 0x0d, 0x0a, 0xff),
  });
  assert.deepEqual(parseSerialHexInput("4154"), { ok: true, bytes: bytes(0x41, 0x54) });
  assert.deepEqual(parseSerialHexInput("   "), { ok: true, bytes: bytes() });
});

test("HEX 输入奇数位或非法字符返回错误", () => {
  assert.deepEqual(parseSerialHexInput("41 5"), { ok: false, error: "oddLength" });
  assert.deepEqual(parseSerialHexInput("4G"), { ok: false, error: "invalidChar" });
});

test("HEX 显示为大写、空格分隔", () => {
  assert.equal(formatSerialHex(bytes(0x0d, 0x0a, 0xff, 0x00)), "0D 0A FF 00");
});

test("文本发送按选择追加行尾，HEX 发送不追加", () => {
  const send = (lineEnding: "none" | "cr" | "lf" | "crlf") =>
    buildSerialSendPayload({ input: "AT", mode: "text", lineEnding });
  assert.deepEqual(send("none"), { ok: true, bytes: text("AT") });
  assert.deepEqual(send("cr"), { ok: true, bytes: text("AT\r") });
  assert.deepEqual(send("lf"), { ok: true, bytes: text("AT\n") });
  assert.deepEqual(send("crlf"), { ok: true, bytes: text("AT\r\n") });
  assert.deepEqual(buildSerialSendPayload({ input: "41 54", mode: "hex", lineEnding: "crlf" }), {
    ok: true,
    bytes: bytes(0x41, 0x54),
  });
});

test("UTF-8 多字节字符跨 chunk 时完整解码", () => {
  const decoder = createSerialStreamDecoder("utf-8");
  const encoded = text("温度");
  assert.equal(decoder.decode(encoded.slice(0, 2)), "");
  assert.equal(decoder.decode(encoded.slice(2)), "温度");
});

test("GBK 多字节字符跨 chunk 时完整解码", () => {
  const decoder = createSerialStreamDecoder("gbk");
  // "温" 的 GBK 编码为 CE C2
  assert.equal(decoder.decode(bytes(0xce)), "");
  assert.equal(decoder.decode(bytes(0xc2, 0x41)), "温A");
});

// --- 快照与事件衔接 -----------------------------------------------------------

test("快照到达前的事件暂存，快照后丢弃 seq ≤ 快照 seq 的部分", () => {
  let log = createSerialChunkLog();
  log = applySerialChunk(log, chunk(3));
  log = applySerialChunk(log, chunk(4));
  assert.deepEqual(log.chunks, []);
  log = applySerialSnapshot(log, snapshot([chunk(1), chunk(2), chunk(3)]));
  assert.deepEqual(
    log.chunks.map((item) => item.seq),
    [1, 2, 3, 4],
  );
  log = applySerialChunk(log, chunk(4));
  log = applySerialChunk(log, chunk(5));
  assert.deepEqual(
    log.chunks.map((item) => item.seq),
    [1, 2, 3, 4, 5],
  );
});

test("渲染缓冲按字节上限淘汰最旧 chunk", () => {
  let log = createSerialChunkLog({ limitBytes: 2 });
  log = applySerialSnapshot(log, snapshot([chunk(1), chunk(2)]));
  log = applySerialChunk(log, chunk(3));
  assert.deepEqual(
    log.chunks.map((item) => item.seq),
    [2, 3],
  );
});

// --- 侧边面板 -----------------------------------------------------------------

test("每次打开串口调试都新建一个未绑定的 serial 标签（多串口）", () => {
  let state = openSerialSidePane(null);
  state = openSerialSidePane(state);
  const tabs = state.tabs.filter((tab) => tab.type === "serial");
  assert.equal(tabs.length, 2);
  assert.notEqual(tabs[0]!.id, tabs[1]!.id);
  assert.ok(tabs.every((tab) => tab.id.startsWith("serial:")));
  assert.equal(state.activeTabId, tabs[1]!.id);
  assert.ok(tabs.every((tab) => tab.type === "serial" && tab.path === undefined));
});

test("绑定串口后标签记住 path，标题只显示串口名（多标签时避免被截断成同样的前缀）", async () => {
  const { bindSerialSidePaneTab } = await import("../src/lib/workspaceSidePane.js");
  const { getSidePaneTabTitle } = await import("../src/app-shell/SidePaneTabTrigger.js");
  const opened = openSerialSidePane(null);
  const tabId = opened.activeTabId;
  const bound = bindSerialSidePaneTab(opened, tabId, "COM3");
  const tab = bound?.tabs.find((item) => item.id === tabId);
  assert.equal(tab?.type === "serial" && tab.path, "COM3");
  const format = ({ id }: { id: string }) => (id === "serial.title" ? "串口调试" : id);
  assert.equal(getSidePaneTabTitle(tab!, format), "COM3");
  const unbound = opened.tabs.find((item) => item.id === tabId)!;
  assert.equal(getSidePaneTabTitle(unbound, format), "串口调试");
  // 未知标签不变
  assert.equal(bindSerialSidePaneTab(opened, "missing", "COM4"), opened);
});

test("serial 标签跟随窗口，在任意 workspace 与对话中可见", () => {
  const stamped = stampSidePaneTabsOwnership(openSerialSidePane(null), {
    ownerTaskId: "task-a",
    workspaceKey: "workspace-a",
  });
  assert.ok(stamped);
  const tab = stamped.tabs.find((item) => item.type === "serial");
  assert.equal(tab?.workspaceKey ?? null, null);
  const visible = getVisibleSidePaneTabs(stamped.tabs, {
    ownerTaskId: "task-b",
    workspaceKey: "workspace-b",
  });
  assert.ok(visible.some((item) => item.type === "serial"));
});

test("新建标签菜单只在平台支持串口时提供串口入口", () => {
  const base = { developerToolsEnabled: false, hasReviewTab: true };
  assert.ok(!resolveOpenTabLauncherItemIds(base).includes("serial"));
  assert.ok(
    resolveOpenTabLauncherItemIds({ ...base, supportsSerialPort: true }).includes("serial"),
  );
});

// --- 参数记忆（全局设置） -----------------------------------------------------

test("全局设置按串口名保存串口参数", async () => {
  const { appSettingsSchema, appSettingsPatchSchema } =
    await import("../../shared/src/validationAppSettings.js");
  const preferences = {
    lastPath: "COM3",
    byPath: {
      COM3: {
        baudRate: 9600,
        dataBits: 8,
        parity: "none",
        stopBits: 1,
        rtscts: false,
        autoReconnect: true,
      },
    },
  };
  const parsed = appSettingsSchema.parse({ serialPortPreferences: preferences });
  assert.deepEqual(parsed.serialPortPreferences, preferences);
  assert.ok(appSettingsPatchSchema.safeParse({ serialPortPreferences: preferences }).success);
  assert.ok(
    !appSettingsPatchSchema.safeParse({
      serialPortPreferences: { byPath: { COM3: { ...preferences.byPath.COM3, baudRate: 0 } } },
    }).success,
  );
});

test("收发计数以快照为基准，只累加快照之后的 chunk；清屏同时归零", async () => {
  const { clearSerialChunkLog } = await import("../src/lib/serial/serialChunkLog.js");
  let log = createSerialChunkLog();
  log = applySerialChunk(log, chunk(2, "tx"));
  log = applySerialChunk(log, chunk(3, "rx"));
  log = applySerialSnapshot(log, {
    ...snapshot([chunk(1), chunk(2, "tx")]),
    stats: { rxBytes: 10, txBytes: 20 },
  });
  assert.deepEqual(log.stats, { rxBytes: 11, txBytes: 20 });
  log = applySerialChunk(log, chunk(4, "tx"));
  assert.deepEqual(log.stats, { rxBytes: 11, txBytes: 21 });
  log = clearSerialChunkLog(log);
  assert.deepEqual(log.stats, { rxBytes: 0, txBytes: 0 });
});

// --- 显示行 -------------------------------------------------------------------

test("无时间戳时同方向相邻 chunk 合并为一行，方向切换另起一行", async () => {
  const { buildSerialDisplayRows } = await import("../src/lib/serial/serialFormat.js");
  const chunks: SerialChunk[] = [
    { seq: 1, at: 1, direction: "tx", source: "user", bytes: text("AT\r\n") },
    { seq: 2, at: 2, direction: "rx", source: "user", bytes: text("O") },
    { seq: 3, at: 3, direction: "rx", source: "user", bytes: text("K\r\n") },
  ];
  const rows = buildSerialDisplayRows(chunks, {
    mode: "text",
    encoding: "utf-8",
    showTimestamp: false,
  });
  assert.deepEqual(
    rows.map((row) => [row.direction, row.text]),
    [
      ["tx", "AT\r\n"],
      ["rx", "OK\r\n"],
    ],
  );
});

test("开启时间戳时每个 chunk 独立成行；HEX 模式按字节显示", async () => {
  const { buildSerialDisplayRows } = await import("../src/lib/serial/serialFormat.js");
  const chunks: SerialChunk[] = [
    { seq: 1, at: 10, direction: "rx", source: "user", bytes: bytes(0x4f) },
    { seq: 2, at: 20, direction: "rx", source: "user", bytes: bytes(0x4b, 0x0a) },
  ];
  const rows = buildSerialDisplayRows(chunks, {
    mode: "hex",
    encoding: "utf-8",
    showTimestamp: true,
  });
  assert.deepEqual(
    rows.map((row) => [row.at, row.text]),
    [
      [10, "4F"],
      [20, "4B 0A"],
    ],
  );
});

test("导出内容逐 chunk 带 ISO 时间与方向", async () => {
  const { buildSerialExportText } = await import("../src/lib/serial/serialFormat.js");
  const chunks: SerialChunk[] = [
    { seq: 1, at: Date.UTC(2026, 0, 1), direction: "tx", source: "user", bytes: text("AT\r\n") },
    {
      seq: 2,
      at: Date.UTC(2026, 0, 1, 0, 0, 1),
      direction: "rx",
      source: "user",
      bytes: bytes(0x4f, 0x4b),
    },
  ];
  assert.equal(
    buildSerialExportText(chunks, { mode: "hex", encoding: "utf-8" }),
    "2026-01-01T00:00:00.000Z TX 41 54 0D 0A\n2026-01-01T00:00:01.000Z RX 4F 4B\n",
  );
  assert.equal(
    buildSerialExportText(chunks.slice(1), { mode: "text", encoding: "utf-8" }),
    "2026-01-01T00:00:01.000Z RX OK\n",
  );
});

test("命令面板只在平台支持串口时提供添加串口标签命令", async () => {
  const { createQuickPickCommands } = await import("../src/quickpick/quickPickCommands.js");
  const noop = () => {};
  const build = (supportsSerialPort: boolean) =>
    createQuickPickCommands({
      allowOpenWorkspace: true,
      canOpenCommunity: false,
      isSidebarVisible: true,
      isLoggedIn: false,
      supportsSerialPort,
      themeTarget: "dark",
      shortcuts: { newTask: "", openWorkspace: "", toggleSidebar: "", toggleTerminal: "" },
      handlers: {
        createTask: noop,
        openWorkspace: noop,
        openSettings: noop,
        openSkillsSettings: noop,
        openMcpSettings: noop,
        switchTheme: noop,
        openFeedback: noop,
        openCommunity: noop,
        openProductDocs: noop,
        toggleSidebar: noop,
        toggleTerminal: noop,
        togglePreview: noop,
        openTerminalTab: noop,
        openBrowserTab: noop,
        openReviewTab: noop,
        openSerialTab: noop,
      },
    }).map((command) => command.id);
  assert.ok(build(true).includes("add-serial-tab"));
  assert.ok(!build(false).includes("add-serial-tab"));
});

// --- 审批卡片预览 ---------------------------------------------------------------

test("serial_write 审批预览给出字节数、转义文本与 HEX，超过 256 字节截断", async () => {
  const { buildSerialPermissionPreview } =
    await import("../src/lib/serial/serialPermissionPreview.js");
  assert.deepEqual(
    buildSerialPermissionPreview("mcp__serial__serial_write", { data: "AT", lineEnding: "crlf" }),
    { kind: "write", bytes: 4, text: "AT\\r\\n", hex: "41 54 0D 0A", truncated: false },
  );
  const long = buildSerialPermissionPreview("mcp__serial__serial_write", { data: "x".repeat(300) });
  assert.equal(long?.kind === "write" && long.bytes, 300);
  assert.equal(long?.kind === "write" && long.text.length, 256);
  assert.equal(long?.kind === "write" && long.truncated, true);
  assert.deepEqual(
    buildSerialPermissionPreview("mcp__serial__serial_write", { data: "4", encoding: "hex" }),
    { kind: "invalid" },
  );
});

test("serial_open/close 预览参数；其它工具不生成预览", async () => {
  const { buildSerialPermissionPreview } =
    await import("../src/lib/serial/serialPermissionPreview.js");
  assert.deepEqual(
    buildSerialPermissionPreview("mcp__serial__serial_open", { path: "COM3", baudRate: 9600 }),
    { kind: "open", path: "COM3", params: "9600 8N1" },
  );
  assert.deepEqual(buildSerialPermissionPreview("mcp__serial__serial_close", {}), {
    kind: "close",
  });
  // 多串口：目标串口来自工具输入的 path
  assert.deepEqual(buildSerialPermissionPreview("mcp__serial__serial_close", { path: "COM5" }), {
    kind: "close",
    path: "COM5",
  });
  const write = buildSerialPermissionPreview("mcp__serial__serial_write", {
    data: "A",
    path: "COM5",
  });
  assert.equal(write?.kind === "write" && write.path, "COM5");
  assert.equal(buildSerialPermissionPreview("mcp__serial__serial_read", {}), null);
  assert.equal(buildSerialPermissionPreview("mcp__other__write", { data: "x" }), null);
});

// --- Agent 来源标注 -------------------------------------------------------------

test("不同 Agent 会话的写入不合并为一行，并保留 sessionId", async () => {
  const { buildSerialDisplayRows } = await import("../src/lib/serial/serialFormat.js");
  const agent = (seq: number, sessionId: string): SerialChunk => ({
    seq,
    at: seq,
    direction: "tx",
    source: "agent",
    sessionId,
    bytes: text(`c${seq}`),
  });
  const rows = buildSerialDisplayRows([agent(1, "s-1"), agent(2, "s-1"), agent(3, "s-2")], {
    mode: "text",
    encoding: "utf-8",
    showTimestamp: false,
  });
  assert.deepEqual(
    rows.map((row) => [row.sessionId, row.text]),
    [
      ["s-1", "c1c2"],
      ["s-2", "c3"],
    ],
  );
});

test("按 sessionId 在各 workspace 的任务缓存中查找会话标题，查不到时回退为 ID 前 8 位", async () => {
  const { resolveSerialAgentSession, formatSerialAgentLabel } =
    await import("../src/lib/serial/serialAgentSession.js");
  const workspaces = {
    "C:/a": { taskListCache: [{ taskId: "other", title: "Other" }] },
    "remote-1": {
      taskListCache: [
        { taskId: "session-abcdef123", title: "Flash firmware", workspacePath: "/home/u/fw" },
      ],
    },
  };
  const found = resolveSerialAgentSession(workspaces as never, "session-abcdef123");
  // 跳转需要任务自身的 workspacePath / workspaceIdentity；缺省时回退到 store 的 workspace key
  assert.deepEqual(found, { title: "Flash firmware", workspacePath: "/home/u/fw" });
  assert.equal(formatSerialAgentLabel(found?.title, "session-abcdef123"), "Flash firmware");
  assert.equal(resolveSerialAgentSession(workspaces as never, "missing"), null);
  assert.equal(formatSerialAgentLabel(undefined, "session-abcdef123"), "session-");
});

test("多个面板同时可见时按引用计数开关 Host 轮询", async () => {
  const { acquireSerialWatch } = await import("../src/hooks/serialWatch.js");
  const calls: boolean[] = [];
  const service = {
    setWatching: async ({ watching }: { watching: boolean }) => void calls.push(watching),
  };
  const releaseA = acquireSerialWatch(service as never);
  const releaseB = acquireSerialWatch(service as never);
  releaseA();
  assert.deepEqual(calls, [true]);
  releaseB();
  releaseB(); // 重复释放幂等
  assert.deepEqual(calls, [true, false]);
});

// --- 多串口：标签的串口选择 -----------------------------------------------------

test("新标签优先预选上次用过且未被占用的串口，否则选第一个空闲串口", async () => {
  const { pickInitialSerialPath } = await import("../src/lib/serial/serialPortChoice.js");
  const ports = [{ path: "COM1" }, { path: "COM2" }, { path: "COM3" }];
  const running = (path: string) => ({ path, status: { state: "open" as const, path } });
  assert.equal(
    pickInitialSerialPath({ tabPath: "COM9", lastPath: "COM1", ports, sessions: [] }),
    "COM9",
  );
  assert.equal(pickInitialSerialPath({ lastPath: "COM2", ports, sessions: [] }), "COM2");
  assert.equal(
    pickInitialSerialPath({ lastPath: "COM1", ports, sessions: [running("COM1")] }),
    "COM2",
  );
  assert.equal(
    pickInitialSerialPath({ ports: [{ path: "COM1" }], sessions: [running("COM1")] }),
    "COM1",
  );
  assert.equal(pickInitialSerialPath({ ports: [], sessions: [] }), undefined);
});

test("串口下拉合并本机串口与运行中会话，并标注运行中", async () => {
  const { buildSerialPortOptions } = await import("../src/lib/serial/serialPortChoice.js");
  const options = buildSerialPortOptions(
    [{ path: "COM1", manufacturer: "FTDI" }, { path: "COM2" }],
    [
      { path: "COM2", status: { state: "open", path: "COM2" } },
      { path: "COM7", status: { state: "disconnected", path: "COM7" } },
    ],
  );
  assert.deepEqual(options, [
    { path: "COM1", manufacturer: "FTDI", running: false },
    { path: "COM2", running: true },
    { path: "COM7", running: true },
  ]);
});
