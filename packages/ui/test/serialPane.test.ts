import assert from "node:assert/strict";
import test from "node:test";
import type { SerialChunk, SerialSnapshot } from "@escode/services";
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

// --- DTR/RTS ---------------------------------------------------------------------

test("serial_set_signals 审批预览给出目标信号或脉冲", async () => {
  const { buildSerialPermissionPreview, describeSerialSignalChange } =
    await import("../src/lib/serial/serialPermissionPreview.js");
  assert.deepEqual(
    buildSerialPermissionPreview("mcp__serial__serial_set_signals", { dtr: false, path: "COM3" }),
    { kind: "signals", path: "COM3", dtr: false },
  );
  assert.deepEqual(
    buildSerialPermissionPreview("mcp__serial__serial_set_signals", { pulse: "esp32" }),
    { kind: "signals", pulse: "esp32" },
  );
  assert.deepEqual(
    buildSerialPermissionPreview("mcp__serial__serial_set_signals", { pulse: "esp32", dtr: true }),
    { kind: "invalid" },
  );
  // 变化描述：已知当前状态时显示 旧→新，未知时只显示目标值
  assert.deepEqual(
    describeSerialSignalChange({ dtr: false, rts: true }, { dtr: true, rts: true }),
    ["DTR 1→0", "RTS 1→1"],
  );
  assert.deepEqual(describeSerialSignalChange({ rts: false }, undefined), ["RTS →0"]);
});

// --- 快捷指令 ---------------------------------------------------------------------

test("快捷指令：增删改与移动", async () => {
  const lib = await import("../src/lib/serial/serialQuickCommands.js");
  let list = lib.addSerialQuickCommand([], {
    name: "AT",
    data: "AT",
    mode: "text",
    lineEnding: "crlf",
  });
  list = lib.addSerialQuickCommand(list, {
    name: "Reset",
    data: "AA 55",
    mode: "hex",
    lineEnding: "none",
  });
  assert.equal(list.length, 2);
  assert.ok(list.every((item) => typeof item.id === "string" && item.id.length > 0));
  list = lib.updateSerialQuickCommand(list, list[0]!.id, { name: "AT test" });
  assert.equal(list[0]!.name, "AT test");
  list = lib.moveSerialQuickCommand(list, list[1]!.id, list[0]!.id);
  assert.deepEqual(
    list.map((item) => item.name),
    ["Reset", "AT test"],
  );
  list = lib.removeSerialQuickCommand(list, list[0]!.id);
  assert.deepEqual(
    list.map((item) => item.name),
    ["AT test"],
  );
});

test("快捷指令：上限 100 条", async () => {
  const lib = await import("../src/lib/serial/serialQuickCommands.js");
  let list: ReturnType<typeof lib.addSerialQuickCommand> = [];
  for (let index = 0; index < 101; index += 1) {
    list = lib.addSerialQuickCommand(list, {
      name: `c${index}`,
      data: "x",
      mode: "text",
      lineEnding: "none",
    });
  }
  assert.equal(list.length, 100);
});

test("快捷指令导入：追加、跳过非法条目、超过上限丢弃并报告数量；导出可再导入", async () => {
  const lib = await import("../src/lib/serial/serialQuickCommands.js");
  const existing = lib.addSerialQuickCommand([], {
    name: "keep",
    data: "k",
    mode: "text",
    lineEnding: "none",
  });
  const json = JSON.stringify([
    { name: "ok", data: "AT", mode: "text", lineEnding: "crlf" },
    { name: "", data: "x", mode: "text", lineEnding: "none" },
    { name: "badmode", data: "x", mode: "binary", lineEnding: "none" },
    { name: "hex", data: "41 42", mode: "hex", lineEnding: "none" },
  ]);
  const result = lib.importSerialQuickCommands(existing, json);
  assert.deepEqual(
    result.commands.map((item) => item.name),
    ["keep", "ok", "hex"],
  );
  assert.equal(result.imported, 2);
  assert.equal(result.skipped, 2);
  assert.equal(result.dropped, 0);
  assert.equal(lib.importSerialQuickCommands([], "not json").skipped, 0);
  assert.equal(lib.importSerialQuickCommands([], "not json").error, "invalidJson");
  const exported = lib.exportSerialQuickCommands(result.commands);
  const roundtrip = lib.importSerialQuickCommands([], exported);
  assert.deepEqual(
    roundtrip.commands.map((item) => [item.name, item.data, item.mode, item.lineEnding]),
    result.commands.map((item) => [item.name, item.data, item.mode, item.lineEnding]),
  );
  const many = JSON.stringify(
    Array.from({ length: 120 }, (_, i) => ({
      name: `n${i}`,
      data: "x",
      mode: "text",
      lineEnding: "none",
    })),
  );
  const capped = lib.importSerialQuickCommands(existing, many);
  assert.equal(capped.commands.length, 100);
  assert.equal(capped.dropped, 21);
});

test("全局设置保存快捷指令列表并校验", async () => {
  const { appSettingsPatchSchema } = await import("../../shared/src/validationAppSettings.js");
  const command = { id: "c1", name: "AT", data: "AT", mode: "text", lineEnding: "crlf" };
  assert.ok(appSettingsPatchSchema.safeParse({ serialQuickCommands: [command] }).success);
  assert.ok(
    !appSettingsPatchSchema.safeParse({ serialQuickCommands: [{ ...command, mode: "bin" }] })
      .success,
  );
  assert.ok(
    !appSettingsPatchSchema.safeParse({
      serialQuickCommands: Array.from({ length: 101 }, () => command),
    }).success,
  );
});

test("快捷指令可携带校验和配置，发送字节与发送栏同一规则", async () => {
  const { appSettingsPatchSchema } = await import("../../shared/src/validationAppSettings.js");
  const lib = await import("../src/lib/serial/serialQuickCommands.js");
  const { formatSerialHex } = await import("../src/lib/serial/serialFormat.js");
  const command = {
    id: "c1",
    name: "frame",
    data: "AA 55 01 02",
    mode: "hex",
    lineEnding: "none",
    checksum: { algorithm: "xor", skip: 2 },
  } as const;
  assert.ok(appSettingsPatchSchema.safeParse({ serialQuickCommands: [command] }).success);
  assert.ok(
    !appSettingsPatchSchema.safeParse({
      serialQuickCommands: [{ ...command, checksum: { algorithm: "md5" } }],
    }).success,
  );
  const payload = lib.buildSerialQuickCommandPayload(command);
  assert.equal(payload.ok ? formatSerialHex(payload.bytes) : "", "AA 55 01 02 03");
  const plain = lib.buildSerialQuickCommandPayload({
    id: "c2",
    name: "AT",
    data: "AT",
    mode: "text",
    lineEnding: "crlf",
  });
  assert.equal(plain.ok ? formatSerialHex(plain.bytes) : "", "41 54 0D 0A");
  const roundtrip = lib.importSerialQuickCommands([], lib.exportSerialQuickCommands([command]));
  assert.deepEqual(roundtrip.commands[0]?.checksum, { algorithm: "xor", skip: 2 });
});

// --- 定时循环发送 -----------------------------------------------------------------

test("循环参数：间隔 ≥10ms，次数留空为无限、填写须为正整数", async () => {
  const { parseSerialLoopInputs } = await import("../src/lib/serial/serialLoopInputs.js");
  assert.deepEqual(parseSerialLoopInputs("100", ""), { ok: true, intervalMs: 100 });
  assert.deepEqual(parseSerialLoopInputs(" 250 ", "5"), { ok: true, intervalMs: 250, count: 5 });
  assert.deepEqual(parseSerialLoopInputs("9", ""), { ok: false, error: "interval" });
  assert.deepEqual(parseSerialLoopInputs("abc", ""), { ok: false, error: "interval" });
  assert.deepEqual(parseSerialLoopInputs("100", "0"), { ok: false, error: "count" });
  assert.deepEqual(parseSerialLoopInputs("100", "1.5"), { ok: false, error: "count" });
});

// --- 接收分帧与帧校验 ---------------------------------------------------------------

function rx(seq: number, at: number, bytes: number[]) {
  return {
    seq,
    at,
    direction: "rx" as const,
    source: "user" as const,
    bytes: new Uint8Array(bytes),
  };
}

test("分帧参数：分隔符支持文本转义与 HEX，长度与间隔须为正整数", async () => {
  const { parseSerialFramingInputs } = await import("../src/lib/serial/serialFraming.js");
  const base = {
    mode: "delimiter",
    delimiter: "\r\n",
    delimiterMode: "text",
    length: "8",
    gapMs: "20",
  } as const;
  const text = parseSerialFramingInputs(base);
  assert.ok(text.ok);
  assert.deepEqual(
    text.ok && text.framing.mode === "delimiter" ? [...text.framing.delimiter] : [],
    [0x0d, 0x0a],
  );
  const escaped = parseSerialFramingInputs({ ...base, delimiter: "\x7E\t\\\\" });
  assert.deepEqual(
    escaped.ok && escaped.framing.mode === "delimiter" ? [...escaped.framing.delimiter] : [],
    [0x7e, 0x09, 0x5c],
  );
  const hex = parseSerialFramingInputs({ ...base, delimiter: "0D 0A", delimiterMode: "hex" });
  assert.deepEqual(
    hex.ok && hex.framing.mode === "delimiter" ? [...hex.framing.delimiter] : [],
    [0x0d, 0x0a],
  );
  assert.deepEqual(parseSerialFramingInputs({ ...base, delimiter: "" }), {
    ok: false,
    error: "delimiter",
  });
  assert.deepEqual(parseSerialFramingInputs({ ...base, delimiter: "0G", delimiterMode: "hex" }), {
    ok: false,
    error: "delimiter",
  });
  assert.deepEqual(parseSerialFramingInputs({ ...base, mode: "length", length: "4" }), {
    ok: true,
    framing: { mode: "length", length: 4 },
  });
  assert.deepEqual(parseSerialFramingInputs({ ...base, mode: "length", length: "0" }), {
    ok: false,
    error: "length",
  });
  assert.deepEqual(parseSerialFramingInputs({ ...base, mode: "gap", gapMs: "50" }), {
    ok: true,
    framing: { mode: "gap", gapMs: 50 },
  });
  assert.deepEqual(parseSerialFramingInputs({ ...base, mode: "gap", gapMs: "x" }), {
    ok: false,
    error: "gap",
  });
});

test("分隔符分帧：跨 chunk 拼接，帧内去掉分隔符，未完成的尾帧单独标记", async () => {
  const { buildSerialFramedRows } = await import("../src/lib/serial/serialFraming.js");
  const chunks = [rx(1, 100, [0x41, 0x42, 0x0d]), rx(2, 110, [0x0a, 0x43, 0x0d, 0x0a, 0x44])];
  const rows = buildSerialFramedRows(chunks, {
    mode: "text",
    encoding: "utf-8",
    framing: { mode: "delimiter", delimiter: new Uint8Array([0x0d, 0x0a]) },
  });
  assert.deepEqual(
    rows.map((row) => [row.text, row.at, row.partial ?? false]),
    [
      ["AB", 100, false],
      ["C", 110, false],
      ["D", 110, true],
    ],
  );
});

test("固定长度与时间间隔分帧；TX 行保持原样且不打断 RX 帧", async () => {
  const { buildSerialFramedRows } = await import("../src/lib/serial/serialFraming.js");
  const tx = {
    seq: 2,
    at: 105,
    direction: "tx" as const,
    source: "user" as const,
    bytes: new Uint8Array([0x31]),
  };
  const byLength = buildSerialFramedRows([rx(1, 100, [1, 2, 3]), tx, rx(3, 110, [4, 5])], {
    mode: "hex",
    encoding: "utf-8",
    framing: { mode: "length", length: 4 },
  });
  assert.deepEqual(
    byLength.map((row) => [row.direction, row.text, row.partial ?? false]),
    [
      ["tx", "31", false],
      ["rx", "01 02 03 04", false],
      ["rx", "05", true],
    ],
  );
  const byGap = buildSerialFramedRows([rx(1, 100, [1]), rx(2, 105, [2]), rx(3, 200, [3])], {
    mode: "hex",
    encoding: "utf-8",
    framing: { mode: "gap", gapMs: 20 },
  });
  // 间隔模式没有结束标记，尾帧视为完整（数据到达即可校验）。
  assert.deepEqual(
    byGap.map((row) => [row.text, row.at, row.partial ?? false]),
    [
      ["01 02", 100, false],
      ["03", 200, false],
    ],
  );
});

test("帧校验：完整帧按末尾校验和比对，失败给出期望值，未完成尾帧不校验", async () => {
  const { buildSerialFramedRows } = await import("../src/lib/serial/serialFraming.js");
  const rows = buildSerialFramedRows(
    [rx(1, 100, [0xaa, 0x01, 0x02, 0x03, 0x0a, 0xaa, 0x01, 0x02, 0x00, 0x0a, 0xaa])],
    {
      mode: "hex",
      encoding: "utf-8",
      framing: { mode: "delimiter", delimiter: new Uint8Array([0x0a]) },
      verify: { algorithm: "xor", skip: 1 },
    },
  );
  assert.deepEqual(
    rows.map((row) => [row.text, row.checksum]),
    [
      ["AA 01 02 03", { ok: true, expected: "03", actual: "03" }],
      ["AA 01 02 00", { ok: false, expected: "03", actual: "00" }],
      ["AA", undefined],
    ],
  );
});

// --- 波形 ---------------------------------------------------------------------------

function rxText(seq: number, at: number, text: string) {
  return rx(seq, at, [...new TextEncoder().encode(text)]);
}

test("波形行解析：数字列与键值对，非法值跳过、空行忽略", async () => {
  const { parseSerialPlotLine } = await import("../src/lib/serial/serialPlot.js");
  assert.deepEqual(parseSerialPlotLine("1.5, -2\t3e2"), [
    ["ch1", 1.5],
    ["ch2", -2],
    ["ch3", 300],
  ]);
  assert.deepEqual(parseSerialPlotLine("1 abc 3"), [
    ["ch1", 1],
    ["ch3", 3],
  ]);
  assert.deepEqual(parseSerialPlotLine("temp:25.5, hum=60 bad:x"), [
    ["temp", 25.5],
    ["hum", 60],
  ]);
  assert.deepEqual(parseSerialPlotLine("   "), []);
});

test("波形缓冲：增量解析跨 chunk 的行，横轴为相对首点的秒数，最多 8 条曲线", async () => {
  const { SerialPlotBuffer } = await import("../src/lib/serial/serialPlot.js");
  const buffer = new SerialPlotBuffer("utf-8");
  buffer.push([rxText(1, 1000, "a:1 b:"), rxText(2, 1500, "2\r\nnot a number\n")]);
  buffer.push([
    rxText(1, 1000, "a:1 b:"),
    rxText(2, 1500, "2\r\nnot a number\n"),
    rxText(3, 3000, "a:3\n"),
  ]);
  assert.deepEqual(buffer.series, ["a", "b"]);
  assert.deepEqual(buffer.rows, [
    { t: 0, a: 1, b: 2 },
    { t: 1.5, a: 3 },
  ]);
  const wide = new SerialPlotBuffer("utf-8");
  wide.push([rxText(1, 0, `${Array.from({ length: 10 }, (_, i) => i).join(",")}\n`)]);
  assert.equal(wide.series.length, 8);
  assert.deepEqual(Object.keys(wide.rows[0]!).sort(), [
    "ch1",
    "ch2",
    "ch3",
    "ch4",
    "ch5",
    "ch6",
    "ch7",
    "ch8",
    "t",
  ]);
});

test("波形缓冲：每条曲线最多保留 2000 点，TX 不参与，缓冲被清空后重新开始", async () => {
  const { SerialPlotBuffer, SERIAL_PLOT_MAX_POINTS } =
    await import("../src/lib/serial/serialPlot.js");
  assert.equal(SERIAL_PLOT_MAX_POINTS, 2000);
  const buffer = new SerialPlotBuffer("utf-8");
  const text = Array.from(
    { length: 2005 },
    (_, i) => `x:${i}${i % 1000 === 0 ? " y:1" : ""}\n`,
  ).join("");
  const tx = {
    seq: 2,
    at: 0,
    direction: "tx" as const,
    source: "user" as const,
    bytes: new TextEncoder().encode("x:999\n"),
  };
  buffer.push([rxText(1, 0, text), tx]);
  assert.equal(buffer.rows.filter((row) => "x" in row).length, 2000);
  assert.equal(buffer.rows.find((row) => "x" in row)!.x, 5);
  // y 只有 3 个点，未超上限：最早的点所在行去掉 x 后仍保留 y
  assert.equal(buffer.rows.filter((row) => "y" in row).length, 3);
  buffer.push([]);
  assert.deepEqual(buffer.rows, []);
  assert.deepEqual(buffer.series, []);
  buffer.push([rxText(5, 9000, "z:1\n")]);
  assert.deepEqual(buffer.rows, [{ t: 0, z: 1 }]);
});

test("波形导出 CSV：表头 t 加曲线名，缺失值留空", async () => {
  const { SerialPlotBuffer, buildSerialPlotCsv } = await import("../src/lib/serial/serialPlot.js");
  const buffer = new SerialPlotBuffer("utf-8");
  buffer.push([rxText(1, 0, "a:1 b:2\n"), rxText(2, 250, "b:3\n")]);
  assert.equal(buildSerialPlotCsv(buffer.series, buffer.rows), "t,a,b\n0,1,2\n0.25,,3\n");
});
