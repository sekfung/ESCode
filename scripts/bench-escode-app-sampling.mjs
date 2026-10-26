// bench-escode-app 的采样与聚合层：进程表发现（Windows CIM / POSIX ps）、子树展开、角色分类、
// 就绪轮询、整树回收、中位数聚合与表格格式化。独立成模块是为了两个文件都留在 400 行以内
// （仓库 lint 约束 max-lines），也便于后续复用（如差分对拍时统计进程布局）。

import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { kill as killProcess } from "node:process";

export const isWindows = process.platform === "win32";
export const sleep = (ms) => new Promise((done) => setTimeout(done, ms));

// Linux 的 `ps -o time=` 只有秒级分辨率，空闲进程在 1s 采样间隔里经常差值为 0。
// 读 /proc/<pid>/stat 的 jiffies（通常 100Hz）才能看到空转 CPU。
const CLK_TCK = (() => {
  if (process.platform !== "linux") return 100;
  try {
    return Number(capture("getconf", ["CLK_TCK"]).trim()) || 100;
  } catch {
    return 100;
  }
})();

function linuxCpuSeconds(pid) {
  try {
    const stat = readFileSync(`/proc/${pid}/stat`, "utf8");
    // comm 字段可能含空格/括号，从其最后一个 ')' 之后切分：fields[11]=utime，fields[12]=stime。
    const fields = stat.slice(stat.lastIndexOf(")") + 2).split(" ");
    const ticks = Number(fields[11]) + Number(fields[12]);
    return Number.isFinite(ticks) ? ticks / CLK_TCK : null;
  } catch {
    return null;
  }
}

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

/** `ps -o time=` 的 `[[DD-]hh:]mm:ss`（Linux 忙碌进程会跨天，如 `2-12:46:17`）。 */
export function parseCpuTime(text) {
  const value = text.trim();
  const parts = value.split("-");
  if (parts.length > 2) throw new Error(`Invalid CPU time: ${text}`);
  let days = 0;
  let clock = parts[0];
  if (parts.length === 2) {
    days = Number(parts[0]);
    clock = parts[1];
    if (!Number.isInteger(days) || days < 0) throw new Error(`Invalid CPU time: ${text}`);
  }
  const fields = clock.split(":").map(Number);
  if (
    fields.length < 2 ||
    fields.length > 3 ||
    fields.some((field) => !Number.isFinite(field) || field < 0)
  )
    throw new Error(`Invalid CPU time: ${text}`);
  return days * 86_400 + fields.reduce((seconds, field) => seconds * 60 + field, 0);
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
        cpuSeconds:
          process.platform === "linux"
            ? (linuxCpuSeconds(Number(pid)) ?? parseCpuTime(time))
            : parseCpuTime(time),
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
export function classify(entry, options = {}) {
  const command = entry.commandLine;
  const name = entry.name.toLowerCase();
  // 安装包主进程的 argv 被 process.title 改写（ps 里只剩产品名），无法按可执行文件路径识别；
  // 由 findAppMain 先确定主进程 pid，再在这里按 pid 命中。
  if (options.mainPid && entry.pid === options.mainPid) return "main";
  if (/escode-cli-rust(\.exe)?(\s|$)/.test(command)) return "agent";
  if (/(escode|zcode)\.cjs/.test(command) && /app-server|--stdio/.test(command))
    return "agent-node";
  if (/(^|\/)escode-host-local[-_0-9]/.test(name)) return "host";
  if (/(^|\/)escode-host(-rust)?(\.exe)?(\s|$)/.test(name)) return "host-rust";
  // Chromium 子进程标记先于进程名判断：安装包子进程的 argv 可能是 `/proc/self/exe`。
  if (/--type=crashpad-handler/.test(command) || name.startsWith("chrome_crashpad")) {
    return "crashpad";
  }
  if (/--type=renderer/.test(command)) return "renderer";
  if (/--type=gpu/.test(command)) return "gpu";
  if (/--utility-sub-type=node\.mojom/.test(command)) return "host";
  if (/--utility-sub-type=network\.mojom/.test(command)) return "network";
  if (/--utility-sub-type=proxy_resolver/.test(command)) return "proxy";
  if (/--type=utility/.test(command)) return "utility";
  // 安装包主进程名是应用可执行文件（linux-unpacked 下为 escode-preview），不是 electron；用可执行
  // 文件路径识别。Electron 41 下 renderer 的 argv 仍显示为 --type=zygote，单独归为 chromium。
  const installed = options.appBinary ? command.includes(options.appBinary) : false;
  if (name.startsWith("electron") || installed) {
    // xvfb-run 包装脚本的命令行也含应用路径，但不是 App 进程。
    if (/xvfb-run/.test(command)) return "other";
    if (/--type=zygote/.test(command)) return "chromium";
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

/** App 进程角色（都落在主进程子树内）；其余为 dev 构建链/系统壳。 */
export const APP_ROLES = new Set([
  "main",
  "renderer",
  "chromium",
  "gpu",
  "utility",
  "host",
  "host-rust",
  "network",
  "proxy",
  "agent",
  "agent-node",
  "cua-helper",
]);

/** Electron 主进程：进程名为 electron* 且不带 --type=（子进程一律带 --type 或 utility-sub-type）。 */
export function isElectronMain(entry, appBinary) {
  if (/--type=/.test(entry.commandLine)) return false;
  if (/xvfb-run/.test(entry.commandLine)) return false;
  if (/^electron/i.test(entry.name)) return true;
  return Boolean(appBinary && entry.commandLine.includes(appBinary));
}

/**
 * 主进程定位。安装包主进程的 argv 被 process.title 改写（只剩产品名），直接按路径找不到；
 * 它的子进程（zygote/utility）仍保留可执行文件路径，因此用 zygote 的 ppid 反查主进程。
 */
export function findAppMain(tree, appBinary) {
  if (!appBinary) return tree.find((entry) => isElectronMain(entry));
  const direct = tree.find((entry) => isElectronMain(entry, appBinary));
  if (direct) return direct;
  const zygote = tree.find(
    (entry) => entry.commandLine.includes(appBinary) && /--type=zygote/.test(entry.commandLine),
  );
  if (zygote) {
    const parent = tree.find((entry) => entry.pid === zygote.ppid);
    if (parent && !/xvfb-run/.test(parent.commandLine)) return parent;
  }
  const child = tree.find((entry) => entry.commandLine.includes(appBinary));
  return (child && tree.find((entry) => entry.pid === child.ppid)) ?? null;
}

function matchesMarker(entry, marker) {
  return (
    entry.commandLine.includes(marker) || entry.name.toLowerCase().includes(marker.toLowerCase())
  );
}

/**
 * 就绪判定。marker="electron-app"：等到真正的主进程与 renderer 都在（dev 下 tsup/vite 起来后
 * electron 才 launch，实测约 173s；用参数文本匹配会把 concurrently 当成就绪，采到构建链）。
 * 安装包模式不走进程名，而是等主进程日志（数据库终态 + 首屏 did-finish-load）。
 */
export function treeReady(tree, marker, appBinary) {
  if (marker === "electron-app") {
    const main = tree.some((entry) => isElectronMain(entry, appBinary));
    const renderer = tree.some(
      (entry) =>
        (/^electron/i.test(entry.name) || (appBinary && entry.commandLine.includes(appBinary))) &&
        /--type=renderer/.test(entry.commandLine),
    );
    return main && renderer;
  }
  return tree.some((entry) => matchesMarker(entry, marker));
}

export const tail = (text) => text.split("\n").slice(-12).join("\n");

function earlyExitError(child, stderr) {
  return new Error(
    `启动命令提前退出（code=${child.exitCode} signal=${child.signalCode}）；stderr 末尾：\n${tail(stderr())}`,
  );
}

/** 轮询到进程树出现就绪标记；子进程提前退出时立即报错并附 stderr 末尾。 */
export async function waitForTree(child, readyMarker, timeoutMs, stderr, appBinary) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (child.exitCode !== null || child.signalCode !== null) {
      throw earlyExitError(child, stderr);
    }
    const tree = subtree(processTable(), child.pid);
    if (treeReady(tree, readyMarker, appBinary)) return tree;
    await sleep(1000);
  }
  throw new Error(
    `启动超时（${timeoutMs / 1000}s）内进程树未出现就绪标记 "${readyMarker}"；stderr 末尾：\n${tail(stderr())}`,
  );
}

/**
 * 安装包就绪：全部日志标记命中才算 ready（数据库终态 + 首屏加载），比进程名轮询更接近
 * 「主界面可用」。100ms 轮询，避免把冷启动向上取整到秒。
 */
export async function waitForReadyLogs(child, patterns, timeoutMs, stderr, output) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (child.exitCode !== null || child.signalCode !== null) {
      throw earlyExitError(child, stderr);
    }
    const text = output();
    if (patterns.every((pattern) => pattern.test(text))) return text;
    await sleep(100);
  }
  throw new Error(
    `启动超时（${timeoutMs / 1000}s）内未出现就绪日志 ${patterns.map(String).join(" / ")}；` +
      `stderr 末尾：\n${tail(stderr())}\nstdout 末尾：\n${tail(output())}`,
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
        rssMiB: (memory.rss ?? 0) / MiB,
      });
    }
  }
  for (const value of Object.values(roleMemory)) {
    value.workingSetMiB = median(value.runs.map((run) => run.workingSetMiB));
    value.privateMiB = median(value.runs.map((run) => run.privateMiB));
    value.rssMiB = median(value.runs.map((run) => run.rssMiB));
    delete value.runs;
  }
  const appPrimaryMiB = median(last.map((sample) => primaryBytes(sample.appMemory ?? {}) / MiB));
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
    appPrimaryMiB,
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
  "就绪 s",
  "退出回收 ms",
  "进程数",
];

export function summaryRows(summary) {
  return Object.entries(summary).map(([key, value]) => [
    key,
    value.primaryMiB.toFixed(1),
    value.appPrimaryMiB.toFixed(1),
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
        (isWindows ? memory.workingSetMiB : memory.rssMiB).toFixed(1),
        isWindows ? memory.privateMiB.toFixed(1) : "—",
      ]);
    }
  }
  return renderTable(
    ["配置", "角色", "归属", "进程数", isWindows ? "工作集 MiB" : "RSS MiB", "私有提交 MiB"],
    rows,
  );
}
