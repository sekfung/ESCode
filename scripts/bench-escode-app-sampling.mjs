// bench-escode-app 的采样与聚合层：进程表发现（Windows CIM / POSIX ps）、子树展开、角色分类、
// 就绪轮询、整树回收、中位数聚合与表格格式化。独立成模块是为了两个文件都留在 400 行以内
// （仓库 lint 约束 max-lines），也便于后续复用（如差分对拍时统计进程布局）。

import { spawnSync } from "node:child_process";
import { kill as killProcess } from "node:process";

export const isWindows = process.platform === "win32";
export const sleep = (ms) => new Promise((done) => setTimeout(done, ms));

export function capture(command, args) {
  const result = spawnSync(command, args, {
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
    windowsHide: true,
  });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    throw new Error(`${command} exited ${result.status}: ${(result.stderr ?? "").slice(0, 400)}`);
  }
  return result.stdout ?? "";
}

function parseCpuTime(text) {
  const fields = text.trim().split(":").map(Number);
  if (fields.some((field) => !Number.isFinite(field))) throw new Error(`Invalid CPU time: ${text}`);
  return fields.reduce((seconds, field) => seconds * 60 + field, 0);
}

/** 全进程表：{ pid, ppid, name, commandLine, memory: { workingSet?, private?, rss? }, cpuSeconds }。 */
export function processTable() {
  if (isWindows) {
    // 单次 CIM 查询拿全量：WorkingSetSize=工作集、PrivatePageCount=私有提交（与 Get-Process 的
    // PrivateMemorySize64 同值，实测 20709376 一致；PageFileUsage 语义不同、数值对不上，不用）、
    // User/KernelModeTime=累计 CPU（100ns 单位）。
    const script = [
      "$ErrorActionPreference = 'SilentlyContinue'",
      "Get-CimInstance Win32_Process | ForEach-Object {",
      "  [pscustomobject]@{ pid = $_.ProcessId; ppid = $_.ParentProcessId; name = $_.Name;",
      "    cmd = $_.CommandLine; workingSet = $_.WorkingSetSize; privateCount = $_.PrivatePageCount;",
      "    cpu = ($_.UserModeTime + $_.KernelModeTime) / 10000000 }",
      "} | ConvertTo-Json -Compress -Depth 3",
    ].join("\n");
    const output = capture("powershell", ["-NoProfile", "-NonInteractive", "-Command", script]);
    if (!output.trim()) return [];
    const rows = JSON.parse(output);
    return (Array.isArray(rows) ? rows : [rows]).map((row) => ({
      pid: Number(row.pid),
      ppid: Number(row.ppid),
      name: String(row.name ?? ""),
      commandLine: String(row.cmd ?? ""),
      memory: {
        workingSet: Number(row.workingSet ?? 0),
        private: Number(row.privateCount ?? 0),
      },
      cpuSeconds: Number(row.cpu ?? 0),
    }));
  }
  return capture("ps", ["-Ao", "pid=,ppid=,rss=,time=,command="])
    .split("\n")
    .map((line) => line.trim())
    .filter(Boolean)
    .map((line) => {
      const match = /^(\d+)\s+(\d+)\s+(\d+)\s+(\S+)\s+(.*)$/.exec(line);
      if (!match) return null;
      const [, pid, ppid, rss, time, commandLine] = match;
      return {
        pid: Number(pid),
        ppid: Number(ppid),
        name: commandLine.split(/\s+/)[0] ?? "",
        commandLine,
        memory: { rss: Number(rss) * 1024 },
        cpuSeconds: parseCpuTime(time),
      };
    })
    .filter(Boolean);
}

/** 从根 pid 向下收集子树（按 ppid 索引，防环）。 */
export function subtree(table, rootPid) {
  const byParent = new Map();
  for (const entry of table) {
    const list = byParent.get(entry.ppid) ?? [];
    list.push(entry);
    byParent.set(entry.ppid, list);
  }
  const byPid = new Map(table.map((entry) => [entry.pid, entry]));
  const seen = new Set();
  const queue = [rootPid];
  const out = [];
  while (queue.length > 0) {
    const pid = queue.shift();
    if (seen.has(pid)) continue;
    seen.add(pid);
    const entry = byPid.get(pid);
    if (entry) out.push(entry);
    for (const child of byParent.get(pid) ?? []) queue.push(child.pid);
  }
  return out;
}

/**
 * 角色分类（按进程名判 Electron，绝不用参数里的 "electron" 字样——dev 的 concurrently 命令行
 * `-n tsup,vite,electron` 会误命中，实测因此把整个构建链当成了 App）。
 * App 角色：main/renderer/gpu/utility/host/network/proxy/agent/agent-node/cua-helper；
 * 非 App（dev 构建链与系统壳）：devtool/shell/pnpm/node/conhost/crashpad/other。
 */
export function classify(entry) {
  const command = entry.commandLine;
  const name = entry.name.toLowerCase();
  if (/escode-cli-rust(\.exe)?(\s|$)/.test(command)) return "agent";
  if (/(escode|zcode)\.cjs/.test(command) && /app-server|--stdio/.test(command))
    return "agent-node";
  if (name.startsWith("electron")) {
    if (/--type=crashpad-handler/.test(command)) return "crashpad";
    if (/--type=renderer/.test(command)) return "renderer";
    if (/--type=gpu/.test(command)) return "gpu";
    if (/--utility-sub-type=node\.mojom/.test(command)) return "host";
    if (/--utility-sub-type=network\.mojom/.test(command)) return "network";
    if (/--utility-sub-type=proxy_resolver/.test(command)) return "proxy";
    if (/--type=utility/.test(command)) return "utility";
    return "main";
  }
  if (/cua helper/i.test(name) || /cua helper/i.test(command)) return "cua-helper";
  if (name === "conhost.exe" || name === "conhost") return "conhost";
  if (name.startsWith("esbuild")) return "devtool";
  if (name === "cmd.exe" || name === "cmd") return "shell";
  if (/pnpm/i.test(command)) return "pnpm";
  if (name.startsWith("node")) return "node";
  return "other";
}

/** App 进程角色（都落在 electron 主进程子树内）；其余为 dev 构建链/系统壳。 */
export const APP_ROLES = new Set([
  "main",
  "renderer",
  "gpu",
  "utility",
  "host",
  "network",
  "proxy",
  "agent",
  "agent-node",
  "cua-helper",
]);

/** Electron 主进程：进程名为 electron* 且不带 --type=（子进程一律带 --type 或 utility-sub-type）。 */
export function isElectronMain(entry) {
  return /^electron/i.test(entry.name) && !/--type=/.test(entry.commandLine);
}

function matchesMarker(entry, marker) {
  return (
    entry.commandLine.includes(marker) || entry.name.toLowerCase().includes(marker.toLowerCase())
  );
}

/**
 * 就绪判定。marker="electron-app"：等到真正的主进程与 renderer 都在（dev 下 tsup/vite 起来后
 * electron 才 launch，实测约 173s；用参数文本匹配会把 concurrently 当成就绪，采到构建链）。
 */
export function treeReady(tree, marker) {
  if (marker === "electron-app") {
    const main = tree.some((entry) => isElectronMain(entry));
    const renderer = tree.some(
      (entry) => /^electron/i.test(entry.name) && /--type=renderer/.test(entry.commandLine),
    );
    return main && renderer;
  }
  return tree.some((entry) => matchesMarker(entry, marker));
}

export const tail = (text) => text.split("\n").slice(-12).join("\n");

/** 轮询到进程树出现就绪标记；子进程提前退出时立即报错并附 stderr 末尾。 */
export async function waitForTree(child, readyMarker, timeoutMs, stderr) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (child.exitCode !== null || child.signalCode !== null) {
      throw new Error(
        `启动命令提前退出（code=${child.exitCode} signal=${child.signalCode}）；stderr 末尾：\n${tail(stderr())}`,
      );
    }
    const tree = subtree(processTable(), child.pid);
    if (treeReady(tree, readyMarker)) return tree;
    await sleep(1000);
  }
  throw new Error(
    `启动超时（${timeoutMs / 1000}s）内进程树未出现就绪标记 "${readyMarker}"；stderr 末尾：\n${tail(stderr())}`,
  );
}

/** 清理尽力而为：目标可能已经退出，非零退出码不算失败，只报 spawn 层面的错误。 */
function bestEffort(command, args) {
  const result = spawnSync(command, args, { stdio: "ignore", windowsHide: true });
  if (result.error) throw result.error;
}

export function killTree(rootPid, tree) {
  if (isWindows) {
    bestEffort("taskkill", ["/PID", String(rootPid), "/T", "/F"]);
    for (const entry of tree) {
      if (entry.pid !== rootPid) bestEffort("taskkill", ["/PID", String(entry.pid), "/F"]);
    }
    return;
  }
  // detached 启动让子进程成为新会话首进程，优先整组回收（与 Rust 侧 isolate/进程树收口同语义）。
  try {
    killProcess(-rootPid, "SIGKILL");
  } catch {}
  for (const entry of tree.slice().reverse()) {
    try {
      killProcess(entry.pid, "SIGKILL");
    } catch {}
  }
}

function median(values) {
  const sorted = values.filter(Number.isFinite).sort((a, b) => a - b);
  if (sorted.length === 0) return 0;
  const middle = Math.floor(sorted.length / 2);
  return sorted.length % 2 === 0 ? (sorted[middle - 1] + sorted[middle]) / 2 : sorted[middle];
}

export const MiB = 1048576;
/** 双口径主指标：Windows 工作集；POSIX RSS（macOS 的 footprint 口径待阶段 1 接入）。 */
export const primaryBytes = (memory) => (isWindows ? memory.workingSet : memory.rss);

/** 每配置多次运行 → { 就绪/退出/双口径内存/CPU 中位数, 进程数与角色峰值, 角色内存, stderr 末尾 }。 */
export function aggregate(runs) {
  const last = runs.map((run) => run.samples.at(-1) ?? { memory: {}, roles: {}, roleMemory: {} });
  const cpuPercents = runs.flatMap((run) =>
    run.samples.map((sample) => sample.cpuPercent).filter(Number.isFinite),
  );
  const appCpuPercents = runs.flatMap((run) =>
    run.samples.map((sample) => sample.appCpuPercent).filter(Number.isFinite),
  );
  const roles = {};
  const roleMemory = {};
  for (const sample of last) {
    for (const [role, count] of Object.entries(sample.roles ?? {})) {
      roles[role] = Math.max(roles[role] ?? 0, count);
    }
    for (const [role, memory] of Object.entries(sample.roleMemory ?? {})) {
      roleMemory[role] ??= { processes: memory.processes, runs: [] };
      roleMemory[role].processes = Math.max(roleMemory[role].processes, memory.processes);
      roleMemory[role].runs.push({
        workingSetMiB: memory.workingSet / MiB,
        privateMiB: memory.private / MiB,
      });
    }
  }
  for (const value of Object.values(roleMemory)) {
    value.workingSetMiB = median(value.runs.map((run) => run.workingSetMiB));
    value.privateMiB = median(value.runs.map((run) => run.privateMiB));
    delete value.runs;
  }
  const appWorkingSetMiB = median(last.map((sample) => (sample.appMemory?.workingSet ?? 0) / MiB));
  const appPrivateMiB = median(last.map((sample) => (sample.appMemory?.private ?? 0) / MiB));
  return {
    runs: runs.length,
    readySeconds: median(
      runs.map((run) => ((run.readyAt ?? run.startedAt) - run.startedAt) / 1000),
    ),
    exitMs: median(runs.map((run) => run.exitMs ?? 0)),
    primaryMiB: median(last.map((sample) => primaryBytes(sample.memory) / MiB)),
    secondaryMiB: median(last.map((sample) => (sample.memory.private ?? 0) / MiB)),
    appWorkingSetMiB,
    appPrivateMiB,
    chainWorkingSetMiB: median(
      last.map(
        (sample) => ((sample.memory?.workingSet ?? 0) - (sample.appMemory?.workingSet ?? 0)) / MiB,
      ),
    ),
    cpuPercent: median(cpuPercents),
    appCpuPercent: median(appCpuPercents),
    processes: Math.max(...last.map((sample) => sample.processCount ?? 0), 0),
    roles,
    roleMemory,
    stderrTail: runs.map((run) => run.stderrTail).find(Boolean),
  };
}

export function renderTable(headers, rows) {
  const widths = headers.map((header, index) =>
    Math.max(header.length, ...rows.map((row) => String(row[index] ?? "").length)),
  );
  const line = (row) =>
    row.map((cell, index) => String(cell ?? "").padEnd(widths[index])).join("  ");
  return [line(headers), line(widths.map((width) => "-".repeat(width))), ...rows.map(line)].join(
    "\n",
  );
}

export const summaryHeaders = [
  "配置",
  isWindows ? "树合计工作集 MiB" : "树合计 RSS MiB",
  isWindows ? "App 工作集 MiB" : "App RSS MiB",
  isWindows ? "App 私有提交 MiB" : "—（POSIX 待接入）",
  "树 CPU %",
  "App CPU %",
  "就绪 s（含 dev 构建）",
  "退出回收 ms",
  "进程数",
];

export function summaryRows(summary) {
  return Object.entries(summary).map(([key, value]) => [
    key,
    value.primaryMiB.toFixed(1),
    value.appWorkingSetMiB.toFixed(1),
    isWindows ? value.appPrivateMiB.toFixed(1) : "—",
    value.cpuPercent.toFixed(1),
    value.appCpuPercent.toFixed(1),
    value.readySeconds.toFixed(1),
    Math.round(value.exitMs),
    value.processes,
  ]);
}

/** 角色分解（进程数 + 双口径内存）：区分 App 进程与 dev 链/系统壳。 */
export function roleBreakdownTable(summary) {
  const rows = [];
  for (const [key, value] of Object.entries(summary)) {
    for (const [role, memory] of Object.entries(value.roleMemory ?? {}).sort((a, b) =>
      a[0].localeCompare(b[0]),
    )) {
      rows.push([
        key,
        role,
        APP_ROLES.has(role) ? "App" : "非 App",
        memory.processes,
        memory.workingSetMiB.toFixed(1),
        isWindows ? memory.privateMiB.toFixed(1) : "—",
      ]);
    }
  }
  return renderTable(["配置", "角色", "归属", "进程数", "工作集 MiB", "私有提交 MiB"], rows);
}
