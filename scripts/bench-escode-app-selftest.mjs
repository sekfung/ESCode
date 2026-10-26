// bench-escode-app 的自检：合成进程树验证采样管线，并静态校验安装包配置/进程识别。
// 独立成模块以保证主脚本留在 lint max-lines 内。
import { writeFile } from "node:fs/promises";
import { join } from "node:path";
import { installedBenchConfigs } from "./bench-escode-app-installed.mjs";
import { classify, isElectronMain, isWindows, parseCpuTime } from "./bench-escode-app-sampling.mjs";

export function assertBenchHelpers() {
  // Linux `ps` 的累计 CPU 格式（含跨天）；回归 2026-10-18 的 `Invalid CPU time: 2-12:46:17`。
  for (const [text, expected] of [
    ["01:02", 62],
    ["1:02:03", 3_723],
    ["2-12:46:17", 218_777],
  ]) {
    const parsed = parseCpuTime(text);
    if (parsed !== expected) {
      throw new Error(`parseCpuTime(${text}) = ${parsed}, expected ${expected}`);
    }
  }
  // 安装包配置与进程识别（不启动桌面）：A/B/C 的 Host/runtime 选择必须互斥，
  // 且安装包主进程不能继续按 electron 进程名判定。
  const fakePaths = {
    app: "/opt/escode/escode-preview",
    glm: "/opt/escode/resources/glm",
    rust: "/opt/escode/resources/glm/escode-cli-rust",
  };
  const installed = installedBenchConfigs(fakePaths);
  if (
    installed.A.env.ESCODE_RUST_HOST !== "0" ||
    installed.A.env.ESCODE_AGENT_SERVER_RUNTIME !== "node" ||
    installed.B.env.ESCODE_RUST_HOST !== "0" ||
    installed.B.env.ESCODE_AGENT_SERVER_RUNTIME !== "escode-cli-rust" ||
    installed.B.env.ESCODE_AGENT_SERVER_COMMAND !== fakePaths.rust ||
    installed.C.env.ESCODE_RUST_HOST !== "1" ||
    installed.C.env.ESCODE_HOST_RUNTIME_LINK !== "embedded"
  ) {
    throw new Error(`[self-test] 安装包配置错误：${JSON.stringify(installed)}`);
  }
  const installedMain = {
    name: fakePaths.app,
    commandLine: `${fakePaths.app} --no-sandbox`,
  };
  const installedRenderer = {
    name: fakePaths.app,
    commandLine: `${fakePaths.app} --type=renderer`,
  };
  if (
    !isElectronMain(installedMain, fakePaths.app) ||
    classify(installedMain, { appBinary: fakePaths.app }) !== "main" ||
    classify(installedRenderer, { appBinary: fakePaths.app }) !== "renderer"
  ) {
    throw new Error("[self-test] 安装包进程识别错误");
  }
}

export async function createSelfTestConfigs(dir) {
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
  return configs;
}

export function evaluateSelfTest(summary) {
  const a = summary["self 32"];
  const b = summary["self 96"];
  // 断言走抗裁剪口径（Windows=私有提交，POSIX=RSS）。合成 buffer 在夹具里被闭包持有，
  // 96 MiB 与 32 MiB 的提交差稳定 >32 MiB；工作集同时打印作交叉检查。
  const commit = (value) => (isWindows ? value.secondaryMiB : value.primaryMiB);
  const ok =
    commit(b) > commit(a) + 32 && a.processes >= 2 && b.processes >= 2 && a.readySeconds < 30;
  return {
    ok,
    message:
      `[self-test] ${ok ? "PASS" : "FAIL"}：提交口径 B=${commit(b).toFixed(1)} MiB > A=${commit(a).toFixed(1)} MiB（+32 MiB 阈值）；` +
      `工作集 A=${a.primaryMiB.toFixed(1)} B=${b.primaryMiB.toFixed(1)} MiB（交叉检查）；` +
      `进程树 A=${a.processes} B=${b.processes}（≥2）`,
  };
}
