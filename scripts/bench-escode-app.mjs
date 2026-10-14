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
//   node scripts/bench-escode-app.mjs --self-test                    # 合成进程树验证管线（无需桌面）
//
// 配置：
//   A：Node Host + Node CLI（pnpm dev:desktop；monorepo dev 默认即 Node，
//      resolveDefaultESCodeAgentCommand 命中 dev 入口时保持 Node 语义）
//   B：Node Host + Rust CLI（pnpm dev:desktop:escode-cli-rust；先 cargo build --release，
//      冷构建可能数分钟，必要时 --launch-timeout 600）
//   C：Rust Host + 内嵌 runtime —— 阶段 2 产出 Rust Host 后接入（当前占位、显式报错）
//
// 场景与边界（骨架已知缺口，随阶段推进补齐）：
//   - idle：就绪=electron 主进程与 renderer 同时在场（dev 下 tsup/vite 起完 electron 才 launch，
//     实测约 173s），再静置 --settle 秒采样；就绪仍不是日志级「主界面可用」信号（待阶段 1 接入）；
//   - 每次运行使用隔离 data dir（--reuse-profile 可关，见参数说明）——A/B 交替写同一 data dir
//     会共享会话与存储，且两个 runtime 的存储格式迁移会互相污染，默认不这么做；
//   - 「全 App」= electron 主进程子树（与 §5 口径一致）；tsup/vite/esbuild 等 dev 构建链单列，
//     不计入 App；空 profile 的首次运行含首启工作（插件缓存安装等），读数偏高属预期；
//   - 空闲态若角色列没有 agent/agent-node，说明该 profile 下 Agent 尚未起进程，
//     此时 A/B 差异只反映 Host/Electron 侧；"打开 workspace 后采样"的驱动场景待阶段 1 接入；
//   - 开发检出（无 ASAR 与签名）口径，与对标仓库报告一致；测量时请退出其它重负载任务。
//   - 采样/聚合/格式化在 `bench-escode-app-sampling.mjs`（仓库 lint 单文件 ≤400 行）。

import { spawn } from "node:child_process";
import { existsSync } from "node:fs";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { parseArgs } from "node:util";
import { quoteArgsForWindowsShell, resolveSpawnRuntimeOptions } from "./spawn-command.mjs";
import {
  aggregate,
  classify,
  isWindows,
  killTree,
  MiB,
  primaryBytes,
  processTable,
  isElectronMain,
  renderTable,
  roleBreakdownTable,
  sleep,
  subtree,
  summaryHeaders,
  summaryRows,
  tail,
  waitForTree,
} from "./bench-escode-app-sampling.mjs";

const root = resolve(import.meta.dirname, "..");
const began = Date.now();

const CONFIGS = {
  A: {
    label: "Node Host + Node CLI（dev:desktop）",
    command: ["pnpm", ["dev:desktop"]],
    readyMarker: "electron-app",
    env: {},
  },
  B: {
    label: "Node Host + Rust CLI（dev:desktop:escode-cli-rust）",
    command: ["pnpm", ["dev:desktop:escode-cli-rust"]],
    readyMarker: "electron-app",
    env: {},
  },
  C: {
    label: "Rust Host + 内嵌 runtime（阶段 2 后接入）",
    unavailable: "Rust Host 尚未落地（阶段 2）；接入时把 command 指向 eshost 启动脚本。",
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
    "reuse-profile": { type: "boolean", default: false },
    "self-test": { type: "boolean", default: false },
    "list-configs": { type: "boolean", default: false },
    keep: { type: "boolean", default: false },
  },
  allowPositionals: false,
});

if (values["list-configs"]) {
  for (const [key, config] of Object.entries(CONFIGS)) {
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
  };
  let stderr = "";
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
    stdio: ["ignore", "ignore", "pipe"],
    shell: runtime.shell === true,
    detached: !isWindows,
    windowsHide: true,
  });
  child.stderr.on("data", (chunk) => {
    stderr = (stderr + chunk).slice(-32 * 1024);
  });

  const launched = { key: configKey, startedAt: Date.now(), samples: [], exitMs: 0 };
  try {
    await waitForTree(
      child,
      config.readyMarker,
      config.launchTimeoutMs ?? launchTimeoutMs,
      () => stderr,
    );
    launched.readyAt = Date.now();
    await sleep(config.settleMs ?? settleMs);
    const deadline = Date.now() + (config.sampleSeconds ?? sampleSeconds) * 1000;
    let previous = null;
    while (Date.now() < deadline) {
      const table = processTable();
      const tree = subtree(table, child.pid);
      // §5「全 App」口径 = electron 主进程子树（含 renderer/utility/agent/辅助 App）；
      // 启动链（cmd/pnpm/node 构建工具）单列，便于与安装包口径对照。
      const appRoot = tree.find((entry) => isElectronMain(entry));
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
        const role = classify(entry);
        const inApp = appPids.has(entry.pid);
        roles[role] = (roles[role] ?? 0) + 1;
        roleMemory[role] ??= { processes: 0, workingSet: 0, private: 0 };
        roleMemory[role].processes += 1;
        roleMemory[role].workingSet += entry.memory.workingSet ?? 0;
        roleMemory[role].private += entry.memory.private ?? 0;
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
  if (stderr.trim()) launched.stderrTail = tail(stderr);
  return launched;
}

// ---- 主流程 ----

let selected;
let selectedKeys;
let selfTestDir = null;
if (values["self-test"]) {
  // 合成进程树：父 node 起子 node，子进程分配固定内存；验证发现/采样/交错/中位数全链路。
  const dir = await mkdtemp(join(tmpdir(), "escode-bench-selftest-"));
  selfTestDir = dir;
  const configs = {};
  for (const mib of [32, 96]) {
    const parentFile = join(dir, `bench-self-test-parent-${mib}.cjs`);
    const childFile = join(dir, `bench-self-test-child-${mib}.cjs`);
    await writeFile(
      // 合成 buffer 必须在闭包里持续被引用：一经分配就再无引用的 buffer 会被 V8 外部内存 GC
      // 回收（实测 96 MiB 超过外部内存阈值先被回收、32 MiB 后回收，读数倒挂），读数会随 GC 时机波动。
      childFile,
      `const buffer = Buffer.alloc(${mib} * 1024 * 1024);\nbuffer.fill(7);\nsetInterval(() => { if (buffer[0] !== 7) process.exit(1); }, 1000);\n`,
    );
    await writeFile(
      parentFile,
      `require("node:child_process").spawn(process.execPath, [${JSON.stringify(childFile)}], { stdio: "ignore" });\nsetInterval(() => {}, 1000);\n`,
    );
    configs[`self ${mib}`] = {
      key: `self ${mib}`,
      label: `self-test（合成 ${mib} MiB 负载）`,
      command: [process.execPath, [parentFile]],
      readyMarker: "bench-self-test",
      settleMs: 1000,
      sampleSeconds: 3,
      isolatedProfile: false,
      env: {},
    };
  }
  selected = configs;
  selectedKeys = Object.keys(configs);
  console.log("[bench-app] self-test：合成进程树，不启动桌面");
} else {
  selected = CONFIGS;
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
if (!values["self-test"] && selectedKeys.includes("B") && !existsSync(rustBinary)) {
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
  summary[key] = { label: selected[key].label, ...aggregate(runs) };
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
  const a = summary["self 32"];
  const b = summary["self 96"];
  // 断言走抗裁剪口径（Windows=私有提交，POSIX=RSS）。合成 buffer 在夹具里被闭包持有，
  // 96 MiB 与 32 MiB 的提交差稳定 >32 MiB；工作集同时打印作交叉检查。
  const commit = (value) => (isWindows ? value.secondaryMiB : value.primaryMiB);
  const ok =
    commit(b) > commit(a) + 32 && a.processes >= 2 && b.processes >= 2 && a.readySeconds < 30;
  console.log(
    `\n[self-test] ${ok ? "PASS" : "FAIL"}：提交口径 B=${commit(b).toFixed(1)} MiB > A=${commit(a).toFixed(1)} MiB（+32 MiB 阈值）；` +
      `工作集 A=${a.primaryMiB.toFixed(1)} B=${b.primaryMiB.toFixed(1)} MiB（交叉检查）；` +
      `进程树 A=${a.processes} B=${b.processes}（≥2）`,
  );
  if (selfTestDir && !values.keep) await rm(selfTestDir, { recursive: true, force: true });
  process.exit(ok ? 0 : 1);
}

await mkdir(outDir, { recursive: true });
const stamp = new Date().toISOString().slice(0, 10);
await writeFile(join(outDir, "summary.json"), JSON.stringify(summary, null, 2));
await writeFile(
  join(outDir, `report-${stamp}.md`),
  [
    "# App 级性能基线（骨架运行）",
    "",
    `- 时间：${new Date().toISOString()}；配置：${selectedKeys.join(" / ")}；交错 ${reps} 轮；场景：${values.scenario}`,
    `- 口径：${isWindows ? "Windows WorkingSet64（工作集，footprint 近似）+ PrivateMemorySize64（私有提交）" : "RSS（macOS footprint 口径待接入）"}；CPU=采样窗口内进程 CPU 时间差折算的单核百分比`,
    "- 方法学：交错执行、每配置取中位数、每轮隔离 data dir（A/B 交替复用同一 data dir 会互相污染存储与会话）、本 harness 不建立调试连接",
    `- 开发检出（无 ASAR 与签名）口径；原始数据见 summary.json；总耗时 ${((Date.now() - began) / 1000).toFixed(0)}s`,
    "",
    "```",
    renderTable(summaryHeaders, summaryRows(summary)),
    "```",
    "",
    "角色分解（App = 应用进程；非 App = dev 构建链/系统壳，不计入 §5「全 App」口径）：",
    "",
    "```",
    roleBreakdownTable(summary),
    "```",
    "",
    "末次采样逐进程明细（末次运行；role=other 需人工判读并回填 classify）：",
    "",
    "```",
    ...Object.entries(summary).flatMap(([key, value]) => [
      `# ${key}`,
      renderTable(
        ["pid", "名称", "角色", "工作集 MiB", "私有提交 MiB", "命令行"],
        (value.lastProcessRows ?? []).map((row) => [
          row.pid,
          row.name,
          row.role,
          row.workingSetMiB,
          row.privateMiB,
          row.commandLine,
        ]),
      ),
      "",
    ]),
    "```",
    "",
    "> 骨架已知缺口：就绪判定基于进程树出现 electron + 静置窗口（不是日志级就绪信号）；" +
      "空闲态若角色列无 agent/agent-node，A/B 差异只反映 Host/Electron 侧；" +
      "多 workspace（N=1/4/8）与流式负载、macOS footprint 口径、C 配置（Rust Host + 内嵌 runtime）待阶段 1/2 接入。",
    "",
  ].join("\n"),
);
console.log(
  `[bench-app] 报告写出：${join(outDir, "summary.json")}，${join(outDir, `report-${stamp}.md`)}`,
);

function positiveInt(text, name) {
  const value = Number(text);
  if (!Number.isInteger(value) || value <= 0) throw new Error(`--${name} 需要正整数，收到 ${text}`);
  return value;
}
