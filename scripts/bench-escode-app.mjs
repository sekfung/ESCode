// App 级性能基线骨架（docs/specs/rust-unified-host.md 阶段 0）。
//
// 目的：为"统一 Host"前后提供同一把尺子——三配置交错采样、按角色分解的双口径内存、
// 同一确定性负载下的启动/退出与常驻 CPU 基线。方法学对齐对标仓库（交错 N 次取中位数、
// 内存双口径、采样时不含调试连接）：
//   - Windows：WorkingSet64（工作集，footprint 近似）与 PrivateMemorySize64（= WMI PrivatePageCount，
//     实测与 Get-Process 同值）双口径；PageFileUsage 语义不同、数值对不上，不用。
//   - macOS/Linux：RSS（ps）；macOS footprint 口径留待阶段 1 接入（骨架注释标注）。
//   - 本 harness 自身不建立任何调试/CDP 连接，也不改 App 的调试开关（与 B 配置的 dev 脚本
//     同样固定 ESCODE_DISABLE_FIXED_REMOTE_DEBUGGING_PORT=1），避免把调试通道开销算进 Host。
//
// 用法：
//   node scripts/bench-escode-app.mjs --list-configs
//   node scripts/bench-escode-app.mjs --configs A,B --reps 3         # 真实桌面（需本机图形会话）
//   node scripts/bench-escode-app.mjs --installed-app <可执行文件> --installed-glm <resources/glm> \
//     --configs A,B,C --reps 3 --out .escode-runtime/app-bench-installed   # 安装包口径
//   node scripts/bench-escode-app.mjs --self-test                    # 合成进程树验证管线（无需桌面）
//
// 配置（dev）：
//   A：Node Host + Node CLI（pnpm dev:desktop，显式 ESCODE_RUST_HOST=0；monorepo dev 默认 Node CLI）
//   B：Node Host + Rust CLI（pnpm dev:desktop:escode-cli-rust，显式 ESCODE_RUST_HOST=0）
//   C：Rust Host + 内嵌 runtime（pnpm dev:desktop，显式 ESCODE_RUST_HOST=1）
//
// 配置（安装包，--installed-app/--installed-glm 或 ESCODE_INSTALLED_APP/GLM）：
//   A：Node Host + Node CLI（ESCODE_RUST_HOST=0 + ESCODE_AGENT_SERVER_RUNTIME=node）
//   B：Node Host + Rust CLI（ESCODE_RUST_HOST=0 + 包内 resources/glm/escode-cli-rust）
//   C：Rust Host + 内嵌 runtime（ESCODE_RUST_HOST=1，runtime-link 默认 embedded）
//
// 场景与边界（骨架已知缺口，随阶段推进补齐）：
//   - idle：dev 就绪=electron 主进程与 renderer 同时在场（dev 下 tsup/vite 起完 electron 才 launch，
//     实测约 173s）；安装包就绪=主进程日志（数据库终态 ready + `[bench-ready]` 首屏 did-finish-load，
//     由 ESCODE_BENCH_READY_LOG=1 受控输出）。随后静置 --settle 秒采样；
//   - 每次运行使用隔离 data dir（--reuse-profile 可关，见参数说明）——A/B 交替写同一 data dir
//     会共享会话与存储，且两个 runtime 的存储格式迁移会互相污染，默认不这么做；
//   - 安装包模式另隔离 HOME/XDG 目录；Linux 无 DISPLAY 时经 xvfb-run 启动（与 installed 演练一致）；
//   - 「全 App」= electron/安装包主进程子树（与 §5 口径一致）；tsup/vite/esbuild 等 dev 构建链单列，
//     不计入 App；空 profile 的首次运行含首启工作（插件缓存安装等），读数偏高属预期；
//   - 空闲态若角色列没有 agent/agent-node/agent，说明该 profile 下 Agent 尚未起进程，
//     此时 A/B/C 差异只反映 Host/Electron 侧；"打开 workspace 后采样"的驱动场景待接入；
//   - 安装包口径（ASAR + 签名状态随包）用于阶段 3 验收；dev 检出读数不可与安装包混用。
//   - 采样/聚合/格式化在 `bench-escode-app-sampling.mjs`（仓库 lint 单文件 ≤400 行）。

import { spawn } from "node:child_process";
import { existsSync } from "node:fs";
import { mkdir, mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { parseArgs } from "node:util";
import { quoteArgsForWindowsShell, resolveSpawnRuntimeOptions } from "./spawn-command.mjs";
import {
  installedBenchConfigs,
  resolveInstalledBenchPaths,
} from "./bench-escode-app-installed.mjs";
import { writeBenchReport } from "./bench-escode-app-report.mjs";
import {
  assertBenchHelpers,
  createSelfTestConfigs,
  evaluateSelfTest,
} from "./bench-escode-app-selftest.mjs";
import {
  aggregate,
  classify,
  isWindows,
  killTree,
  MiB,
  primaryBytes,
  processTable,
  findAppMain,
  renderTable,
  roleBreakdownTable,
  sleep,
  subtree,
  summaryHeaders,
  summaryRows,
  tail,
  waitForReadyLogs,
  waitForTree,
} from "./bench-escode-app-sampling.mjs";

const root = resolve(import.meta.dirname, "..");
const began = Date.now();

const CONFIGS = {
  A: {
    label: "Node Host + Node CLI（dev:desktop）",
    command: ["pnpm", ["dev:desktop"]],
    readyMarker: "electron-app",
    // 默认已翻转 Rust Host；A 必须显式回退 Node Host，否则标签与实际链路不符。
    env: { ESCODE_RUST_HOST: "0", ESCODE_AGENT_SERVER_RUNTIME: "node" },
  },
  B: {
    label: "Node Host + Rust CLI（dev:desktop:escode-cli-rust）",
    command: ["pnpm", ["dev:desktop:escode-cli-rust"]],
    readyMarker: "electron-app",
    // dev 脚本负责 Rust CLI 的构建与命令；Host 侧显式固定 Node，避免默认翻转后变成 Rust Host。
    env: { ESCODE_RUST_HOST: "0" },
  },
  C: {
    label: "Rust Host + 内嵌 runtime（dev:desktop）",
    command: ["pnpm", ["dev:desktop"]],
    readyMarker: "electron-app",
    env: { ESCODE_RUST_HOST: "1" },
  },
};

const rustBinary = resolve(
  root,
  `apps/escode-cli-rust/target/release/escode-cli-rust${isWindows ? ".exe" : ""}`,
);

const { values } = parseArgs({
  options: {
    configs: { type: "string", default: "A,B" },
    reps: { type: "string", default: "3" },
    settle: { type: "string", default: "20" },
    sample: { type: "string", default: "10" },
    cooldown: { type: "string", default: "5" },
    "launch-timeout": { type: "string", default: "420" },
    out: { type: "string", default: ".escode-runtime/app-bench" },
    scenario: { type: "string", default: "idle" },
    "installed-app": { type: "string" },
    "installed-glm": { type: "string" },
    "reuse-profile": { type: "boolean", default: false },
    "self-test": { type: "boolean", default: false },
    "list-configs": { type: "boolean", default: false },
    keep: { type: "boolean", default: false },
  },
  allowPositionals: false,
});

// 安装包模式：显式参数优先，其次 CI 写入的 ESCODE_INSTALLED_APP/GLM。
const installedPaths =
  values["installed-app"] || values["installed-glm"] || process.env.ESCODE_INSTALLED_APP?.trim()
    ? resolveInstalledBenchPaths({
        app: values["installed-app"],
        glm: values["installed-glm"],
      })
    : null;
const activeConfigs = installedPaths ? installedBenchConfigs(installedPaths) : CONFIGS;

if (values["list-configs"]) {
  console.log(
    installedPaths ? `安装包：${installedPaths.app}` : "dev 检出（A/B/C 均启动 dev:desktop）",
  );
  for (const [key, config] of Object.entries(activeConfigs)) {
    console.log(`${key}: ${config.label}${config.unavailable ? `（${config.unavailable}）` : ""}`);
  }
  process.exit(0);
}

const reps = positiveInt(values.reps, "reps");
const settleMs = positiveInt(values.settle, "settle") * 1000;
const sampleSeconds = positiveInt(values.sample, "sample");
const cooldownMs = positiveInt(values.cooldown, "cooldown") * 1000;
const launchTimeoutMs = positiveInt(values["launch-timeout"], "launch-timeout") * 1000;
const outDir = resolve(root, values.out);

// ---- 生命周期：启动 → 就绪 → 静置 → 采样 → 杀树 ----

async function waitExit(child, timeoutMs = 15_000) {
  if (child.exitCode !== null || child.signalCode !== null) return;
  await Promise.race([new Promise((done) => child.once("close", done)), sleep(timeoutMs)]);
}

async function measure(config, configKey, profileBase) {
  const useIsolatedProfile = config.isolatedProfile !== false;
  const profile = join(profileBase, `${configKey}-${Date.now().toString(36)}`);
  if (useIsolatedProfile) await mkdir(profile, { recursive: true });
  const isolatedProfile = {
    ESCODE_DATA_BASE_DIR: join(profile, "app"),
    ESCODE_DESKTOP_APPLICATION_NAME: "ESCode Bench",
    ESCODE_DESKTOP_USER_DATA_DIR: join(profile, "electron"),
    ESCODE_DESKTOP_SESSION_DATA_DIR: join(profile, "electron-session"),
    // 安装包读取用户级 provider/插件目录；不隔离 HOME 会把真实账号配置带进读数。
    ...(config.installed
      ? {
          HOME: join(profile, "home"),
          USERPROFILE: join(profile, "home"),
          XDG_CONFIG_HOME: join(profile, "home", ".config"),
          XDG_DATA_HOME: join(profile, "home", ".local", "share"),
          XDG_CACHE_HOME: join(profile, "home", ".cache"),
        }
      : {}),
  };
  if (config.installed) {
    await Promise.all([
      mkdir(join(profile, "home"), { recursive: true }),
      mkdir(join(profile, "home", ".config"), { recursive: true }),
      mkdir(join(profile, "home", ".local", "share"), { recursive: true }),
      mkdir(join(profile, "home", ".cache"), { recursive: true }),
    ]);
  }
  let stderr = "";
  let stdout = "";
  let output = "";
  const runtime = resolveSpawnRuntimeOptions(config.command[0]);
  const args = runtime.shell ? quoteArgsForWindowsShell(config.command[1]) : config.command[1];
  const child = spawn(config.command[0], args, {
    cwd: root,
    env: {
      ...process.env,
      ESCODE_DISABLE_FIXED_REMOTE_DEBUGGING_PORT: "1",
      ...(useIsolatedProfile ? isolatedProfile : {}),
      ...config.env,
    },
    stdio: ["ignore", "pipe", "pipe"],
    shell: runtime.shell === true,
    detached: !isWindows,
    windowsHide: true,
  });
  child.stderr.on("data", (chunk) => {
    stderr = (stderr + chunk).slice(-32 * 1024);
    output = (output + chunk).slice(-128 * 1024);
  });
  child.stdout.on("data", (chunk) => {
    stdout = (stdout + chunk).slice(-32 * 1024);
    output = (output + chunk).slice(-128 * 1024);
  });

  const launched = { key: configKey, startedAt: Date.now(), samples: [], exitMs: 0 };
  try {
    if (config.readyLogs) {
      await waitForReadyLogs(
        child,
        config.readyLogs,
        config.launchTimeoutMs ?? launchTimeoutMs,
        () => stderr,
        () => output,
      );
      launched.readyKind = "logs";
    } else {
      await waitForTree(
        child,
        config.readyMarker,
        config.launchTimeoutMs ?? launchTimeoutMs,
        () => stderr,
        config.appBinary,
      );
      launched.readyKind = "tree";
    }
    launched.readyAt = Date.now();
    await sleep(config.settleMs ?? settleMs);
    const deadline = Date.now() + (config.sampleSeconds ?? sampleSeconds) * 1000;
    let previous = null;
    while (Date.now() < deadline) {
      const table = processTable();
      const tree = subtree(table, child.pid);
      // §5「全 App」口径 = electron/安装包主进程子树（含 renderer/utility/agent/辅助 App）；
      // 启动链（cmd/pnpm/node 构建工具/xvfb-run）单列，便于与安装包口径对照。
      const appRoot = findAppMain(tree, config.appBinary);
      const appTree = appRoot ? subtree(table, appRoot.pid) : [];
      const appPids = new Set(appTree.map((entry) => entry.pid));
      const roles = {};
      const roleMemory = {};
      const memory = { workingSet: 0, private: 0, rss: 0 };
      const appMemory = { workingSet: 0, private: 0, rss: 0 };
      const rows = [];
      let cpuSeconds = 0;
      let appCpuSeconds = 0;
      let appProcesses = 0;
      for (const entry of tree) {
        const role = classify(entry, { appBinary: config.appBinary, mainPid: appRoot?.pid });
        const inApp = appPids.has(entry.pid);
        roles[role] = (roles[role] ?? 0) + 1;
        roleMemory[role] ??= { processes: 0, workingSet: 0, private: 0, rss: 0 };
        roleMemory[role].processes += 1;
        roleMemory[role].workingSet += entry.memory.workingSet ?? 0;
        roleMemory[role].private += entry.memory.private ?? 0;
        roleMemory[role].rss += entry.memory.rss ?? 0;
        memory.workingSet += entry.memory.workingSet ?? 0;
        memory.private += entry.memory.private ?? 0;
        memory.rss += entry.memory.rss ?? 0;
        cpuSeconds += entry.cpuSeconds;
        if (inApp) {
          appProcesses += 1;
          appCpuSeconds += entry.cpuSeconds;
          appMemory.workingSet += entry.memory.workingSet ?? 0;
          appMemory.private += entry.memory.private ?? 0;
          appMemory.rss += entry.memory.rss ?? 0;
        }
        rows.push({
          pid: entry.pid,
          name: entry.name,
          role,
          area: inApp ? "app" : "chain",
          workingSetMiB: ((entry.memory.workingSet ?? 0) / MiB).toFixed(1),
          rssMiB: ((entry.memory.rss ?? 0) / MiB).toFixed(1),
          privateMiB: ((entry.memory.private ?? 0) / MiB).toFixed(1),
          commandLine: entry.commandLine.slice(0, 150),
        });
      }
      const tick = {
        at: Date.now(),
        memory,
        appMemory,
        cpuSeconds,
        appCpuSeconds,
        roles,
        roleMemory,
        processCount: tree.length,
        appProcesses,
        rows,
      };
      if (previous) {
        const wallSeconds = (tick.at - previous.at) / 1000;
        tick.cpuPercent =
          wallSeconds > 0 ? ((tick.cpuSeconds - previous.cpuSeconds) / wallSeconds) * 100 : 0;
        tick.appCpuPercent =
          wallSeconds > 0 ? ((tick.appCpuSeconds - previous.appCpuSeconds) / wallSeconds) * 100 : 0;
      }
      previous = tick;
      launched.samples.push(tick);
      await sleep(1000);
    }
  } finally {
    const exitBegan = Date.now();
    killTree(child.pid, subtree(processTable(), child.pid));
    await waitExit(child);
    launched.exitMs = Date.now() - exitBegan;
  }
  launched.durationMs = Date.now() - launched.startedAt;
  // 冷启动阶段（T0→T5）：只在安装包 + bench 标记下由 renderer 输出。
  const startupPhase = /\[bench-startup\]\s+([^"\n]+)/.exec(output);
  if (startupPhase) launched.startupPhases = startupPhase[1].trim();
  if (stderr.trim()) launched.stderrTail = tail(stderr);
  if (stdout.trim()) launched.stdoutTail = tail(stdout);
  return launched;
}

// ---- 主流程 ----

let selected;
let selectedKeys;
let selfTestDir = null;
if (values["self-test"]) {
  assertBenchHelpers();
  // 合成进程树：父 node 起子 node，子进程分配固定内存；验证发现/采样/交错/中位数全链路。
  selfTestDir = await mkdtemp(join(tmpdir(), "escode-bench-selftest-"));
  selected = await createSelfTestConfigs(selfTestDir);
  selectedKeys = Object.keys(selected);
  console.log("[bench-app] self-test：合成进程树，不启动桌面");
} else {
  selected = activeConfigs;
  selectedKeys = values.configs
    .split(",")
    .map((key) => key.trim().toUpperCase())
    .filter(Boolean);
}

for (const key of selectedKeys) {
  const config = selected[key];
  if (!config) throw new Error(`未知配置 ${key}`);
  if (config.unavailable) throw new Error(`配置 ${key} 不可用：${config.unavailable}`);
}
if (
  !values["self-test"] &&
  !installedPaths &&
  selectedKeys.includes("B") &&
  !existsSync(rustBinary)
) {
  console.warn(
    `[bench-app] 未找到 Rust release 二进制（${rustBinary}），B 首次运行会先 cargo build --release，` +
      "可能数分钟；必要时调大 --launch-timeout。",
  );
}

const profileBase = values["reuse-profile"]
  ? join(outDir, "profiles", "shared")
  : join(outDir, "profiles");

console.log(
  `[bench-app] 配置 ${selectedKeys.join(" / ")}，交错 ${reps} 轮，场景 ${values["self-test"] ? "self-test" : values.scenario}`,
);
const results = new Map();
for (let rep = 1; rep <= reps; rep++) {
  for (const [index, key] of selectedKeys.entries()) {
    const run = await measure(selected[key], key, profileBase);
    const list = results.get(key) ?? [];
    list.push(run);
    results.set(key, list);
    const last = run.samples.at(-1);
    console.log(
      `[bench-app] rep ${rep}/${reps} ${key}: ${last ? (primaryBytes(last.memory) / MiB).toFixed(1) : "?"} MiB, ` +
        `${run.samples.length} 采样, ready ${((run.readyAt ?? run.startedAt) - run.startedAt) / 1000}s, exit ${run.exitMs}ms`,
    );
    if (index < selectedKeys.length - 1 || rep < reps) await sleep(cooldownMs);
  }
}

const summary = {};
for (const [key, runs] of results) {
  summary[key] = {
    label: selected[key].label,
    readyKind: runs.at(-1)?.readyKind,
    startupPhases: runs.at(-1)?.startupPhases,
    ...aggregate(runs),
  };
  // 末次运行的末次采样逐进程明细（诊断用：未分类进程/角色口径核对）。
  summary[key].lastProcessRows = runs.at(-1)?.samples.at(-1)?.rows ?? [];
}
console.log("\n" + renderTable(summaryHeaders, summaryRows(summary)));
console.log(
  "\n角色分解（App = 应用进程；非 App = dev 构建链/系统壳）：\n" + roleBreakdownTable(summary),
);
for (const [key, value] of Object.entries(summary)) {
  if (value.stderrTail) console.log(`\n[bench-app] ${key} stderr 末尾：\n${value.stderrTail}`);
}

if (values["self-test"]) {
  const { ok, message } = evaluateSelfTest(summary);
  console.log(`\n${message}`);
  if (selfTestDir && !values.keep) await rm(selfTestDir, { recursive: true, force: true });
  process.exit(ok ? 0 : 1);
}

await mkdir(outDir, { recursive: true });
const written = await writeBenchReport({
  outDir,
  summary,
  selectedKeys,
  reps,
  scenario: values.scenario,
  installedApp: installedPaths?.app,
  began,
});
console.log(`[bench-app] 报告写出：${written.summaryPath}，${written.reportPath}`);

function positiveInt(text, name) {
  const value = Number(text);
  if (!Number.isInteger(value) || value <= 0) throw new Error(`--${name} 需要正整数，收到 ${text}`);
  return value;
}
