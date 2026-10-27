import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";

// App 级「全进程」采样：Electron 主进程子树（renderer/gpu/utility/Host/Agent）。
// `app.getAppMetrics()` 只覆盖 Electron 自身进程，Agent（Node bundle / escode-cli-rust）
// 是 Host 的子进程，不在其中；§5 的「4 路流式全进程 CPU」必须把它算进来。
// 与 scripts/bench-escode-app-sampling.mjs 同口径（Linux 用 /proc jiffies，Windows 用 CIM），
// 但保持独立实现，避免 E2E 工程依赖仓库脚本的类型。

export interface ProcessTreeSample {
  at: number;
  cpuPercent: number;
  workingSetMiB: number;
  processes: Array<{
    pid: number;
    name: string;
    commandLine: string;
    cpuSeconds: number;
    bytes: number;
  }>;
}

const isWindows = process.platform === "win32";

function capture(command: string, args: string[]): string {
  return execFileSync(command, args, {
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
    windowsHide: true,
  });
}

function parseCpuTime(text: string): number {
  const [dayPart, clockPart] = text.includes("-") ? text.split("-", 2) : [undefined, text];
  const days = dayPart ? Number(dayPart) : 0;
  const fields = (clockPart ?? "").split(":").map(Number);
  if (fields.length < 2 || fields.length > 3 || fields.some((value) => !Number.isFinite(value))) {
    return 0;
  }
  return days * 86_400 + fields.reduce((seconds, value) => seconds * 60 + value, 0);
}

const CLK_TCK = (() => {
  if (process.platform !== "linux") return 100;
  try {
    return Number(capture("getconf", ["CLK_TCK"]).trim()) || 100;
  } catch {
    return 100;
  }
})();

function linuxCpuSeconds(pid: number): number | null {
  try {
    const stat = readFileSync(`/proc/${pid}/stat`, "utf8");
    const fields = stat.slice(stat.lastIndexOf(")") + 2).split(" ");
    const ticks = Number(fields[11]) + Number(fields[12]);
    return Number.isFinite(ticks) ? ticks / CLK_TCK : null;
  } catch {
    return null;
  }
}

interface RawProcess {
  pid: number;
  ppid: number;
  name: string;
  commandLine: string;
  bytes: number;
  cpuSeconds: number;
}

export function readProcessTable(): RawProcess[] {
  if (isWindows) {
    const script = [
      "$ErrorActionPreference = 'SilentlyContinue'",
      "Get-CimInstance Win32_Process | ForEach-Object {",
      "  [pscustomobject]@{ pid = $_.ProcessId; ppid = $_.ParentProcessId; name = $_.Name; cmd = $_.CommandLine;",
      "    workingSet = $_.WorkingSetSize; cpu = ($_.UserModeTime + $_.KernelModeTime) / 10000000 }",
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
      bytes: Number(row.workingSet ?? 0),
      cpuSeconds: Number(row.cpu ?? 0),
    }));
  }
  return capture("ps", ["-Ao", "pid=,ppid=,rss=,time=,command="])
    .split("\n")
    .map((line) => line.trim())
    .filter(Boolean)
    .flatMap((line) => {
      const match = /^(\d+)\s+(\d+)\s+(\d+)\s+(\S+)\s+(.*)$/u.exec(line);
      if (!match) return [];
      const [, pid, ppid, rss, time, commandLine] = match;
      const elapsed = time ?? "";
      const command = commandLine ?? "";
      const cpuSeconds =
        process.platform === "linux"
          ? (linuxCpuSeconds(Number(pid)) ?? parseCpuTime(elapsed))
          : parseCpuTime(elapsed);
      return [
        {
          pid: Number(pid),
          ppid: Number(ppid),
          name: command.split(/\s+/u)[0] ?? "",
          commandLine: command,
          bytes: Number(rss) * 1024,
          cpuSeconds,
        },
      ];
    });
}

function subtree(table: RawProcess[], rootPid: number): RawProcess[] {
  const byParent = new Map<number, RawProcess[]>();
  const byPid = new Map<number, RawProcess>();
  for (const entry of table) {
    byPid.set(entry.pid, entry);
    const children = byParent.get(entry.ppid) ?? [];
    children.push(entry);
    byParent.set(entry.ppid, children);
  }
  const seen = new Set<number>();
  const queue = [rootPid];
  const out: RawProcess[] = [];
  while (queue.length > 0) {
    const pid = queue.shift();
    if (pid === undefined || seen.has(pid)) continue;
    seen.add(pid);
    const entry = byPid.get(pid);
    if (entry) out.push(entry);
    for (const child of byParent.get(pid) ?? []) queue.push(child.pid);
  }
  return out;
}

/** 采样一次主进程子树；previous 用于把累计 CPU 秒差折算成百分比。 */
export function sampleProcessTree(
  mainPid: number,
  previous?: { at: number; cpuSeconds: number },
): ProcessTreeSample & { cpuSeconds: number } {
  const tree = subtree(readProcessTable(), mainPid);
  const at = Date.now();
  const cpuSeconds = tree.reduce((sum, entry) => sum + entry.cpuSeconds, 0);
  const wallSeconds = previous ? (at - previous.at) / 1000 : 0;
  const cpuPercent =
    previous && wallSeconds > 0 ? ((cpuSeconds - previous.cpuSeconds) / wallSeconds) * 100 : 0;
  return {
    at,
    cpuSeconds,
    cpuPercent,
    workingSetMiB: tree.reduce((sum, entry) => sum + entry.bytes, 0) / 1024 / 1024,
    processes: tree.map((entry) => ({
      pid: entry.pid,
      name: entry.name,
      commandLine: entry.commandLine.slice(0, 200),
      cpuSeconds: entry.cpuSeconds,
      bytes: entry.bytes,
    })),
  };
}

/**
 * 主进程是否仍存活。僵尸（Z/defunct）视为已退出：Electron 退出后父进程（chromedriver/wdio）
 * 未必立刻 reap，`ps` 里仍能看到 pid，按存活判断会让退出测量永远超时。
 */
export function isProcessAlive(pid: number): boolean {
  if (isWindows) {
    return readProcessTable().some((entry) => entry.pid === pid);
  }
  try {
    const stat = readFileSync(`/proc/${pid}/stat`, "utf8");
    const stateIndex = stat.lastIndexOf(")") + 2;
    const state = stat.slice(stateIndex, stateIndex + 1);
    return state !== "Z" && state !== "X";
  } catch {
    if (process.platform === "linux") return false;
  }
  try {
    const output = capture("ps", ["-o", "stat=", "-p", String(pid)]).trim();
    return output !== "" && !output.startsWith("Z");
  } catch {
    return false;
  }
}
