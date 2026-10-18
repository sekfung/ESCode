import { spawn, spawnSync } from "node:child_process";
import { existsSync, realpathSync } from "node:fs";
import { tmpdir } from "node:os";
import { readdir } from "node:fs/promises";
import { resolve } from "node:path";

const root = resolve(import.meta.dirname, "..");
// 修复：Windows runner 的临时目录是 8.3 短路径（C:\Users\RUNNER~1\...）。用例与 runtime 混用短名和
// 长名时，路径归一化对不上（差分误报）；Node 子进程对短名目录 fs.watch 还会触发 libuv 断言
// `!_wcsnicmp(filename, dir, dirlen)` 崩溃。入口统一把 TEMP/TMP 换成长路径，所有子进程
// （Node 的 os.tmpdir() 与 Rust 的 tempfile）都继承同一份长路径临时目录。
if (process.platform === "win32") {
  const longTemp = realpathSync.native(tmpdir());
  process.env.TEMP = longTemp;
  process.env.TMP = longTemp;
}
// CI 与慢机器：每个用例都会起 runtime + 本地模型服务，并发跑会被接收超时误伤；ESCODE_TEST_SERIAL=1 时串行。
// 慢机（debug 的 Windows 尤其）建议再设 ESCODE_TEST_REQUEST_MS=60000 与 ESCODE_TEST_EXIT_MS=15000：
// 默认 5s 请求预算与 4s 关闭预算不够用（与 rust-runtime-ci.yml 的 env 一致，workflow 系列用例实测需要 ~20s+）。
const serial = process.env.ESCODE_TEST_SERIAL === "1";
// CI 分片（ESCODE_TEST_SHARD="i/n"）：App 集成套件串行约 37 分钟（Windows），按文件名排序后轮转分片并行。
// 第 1 片额外承担与分片无关的检查（生成资产、类型检查、cargo 测试）；未设置时等同单片全量。
const [shardIndex, shardCount] = (process.env.ESCODE_TEST_SHARD ?? "1/1").split("/").map(Number);
if (!(shardCount >= 1 && shardIndex >= 1 && shardIndex <= shardCount))
  throw new Error(`Invalid ESCODE_TEST_SHARD: ${process.env.ESCODE_TEST_SHARD}`);
const primary = shardIndex === 1;
async function run(command, args, env = {}) {
  await new Promise((done, fail) => {
    const child = spawn(command, args, {
      cwd: root,
      stdio: "inherit",
      env: {
        ...process.env,
        TSX_TSCONFIG_PATH: resolve(root, "packages/services/tests/tsconfig.escode-cli-rust.json"),
        ...env,
      },
    });
    child.once("error", fail);
    child.once("close", (code, signal) =>
      code === 0 ? done() : fail(new Error(`${command} failed: ${signal ?? code}`)),
    );
  });
}
await run(process.execPath, [
  "apps/escode-cli/packages/dynamic-workflow/scripts/generate-libs.mjs",
]);
// 差分与导入用例直接 import TS bootstrap 源码，其依赖包走 dist 入口；干净检出时必须先构建，
// 否则只报 ERR_MODULE_NOT_FOUND。经 npm_execpath 调 pnpm，避免 Windows 上 pnpm.cmd 需要 shell。
if (!process.env.npm_execpath) throw new Error("Run via `pnpm test:escode-cli-rust`");
await run(process.execPath, [
  process.env.npm_execpath,
  "--dir",
  "apps/escode-cli",
  "--filter",
  "@escode/bootstrap^...",
  "build",
]);
// 生成资产漂移检查只在第 1 片跑（与分片无关，跑一次即可）。
if (primary) {
  await run(process.execPath, [
    "--import",
    "tsx",
    "scripts/generate-escode-cli-rust-prompt.mjs",
    "--check",
  ]);
  await run(process.execPath, [
    "--import",
    "tsx",
    "scripts/generate-escode-cli-rust-tool-schemas.mjs",
    "--check",
  ]);
  await run(process.execPath, [
    "--import",
    "tsx",
    "scripts/generate-escode-cli-rust-permission-matrix.mjs",
    "--check",
  ]);
  await run(process.execPath, [
    "--import",
    "tsx",
    "scripts/generate-escode-cli-rust-rewind-branch-corpus.mjs",
    "--check",
  ]);
  await run(process.execPath, [
    "--import",
    "tsx",
    "scripts/generate-escode-cli-rust-bash-readonly-corpus.mjs",
    "--check",
  ]);
  await run(process.execPath, [
    "--import",
    "tsx",
    "scripts/generate-escode-cli-rust-command-schema.mjs",
    "--check",
  ]);
  await run(process.execPath, [
    "--import",
    "tsx",
    "scripts/generate-escode-cli-rust-bash-policies.mjs",
    "--check",
  ]);
  await run(process.execPath, [
    "--import",
    "tsx",
    "scripts/generate-escode-cli-rust-bash-rule-corpus.mjs",
    "--check",
  ]);
  await run(process.execPath, [
    "--import",
    "tsx",
    "scripts/generate-escode-cli-rust-git-safety-corpus.mjs",
    "--check",
  ]);
  await run(process.execPath, [
    "--import",
    "tsx",
    "scripts/generate-escode-cli-rust-proxy-corpus.mjs",
    "--check",
  ]);
  await run(process.execPath, [
    "--import",
    "tsx",
    "scripts/generate-escode-cli-rust-init-prompt-corpus.mjs",
    "--check",
  ]);
  await run(process.execPath, [
    "--import",
    "tsx",
    "scripts/generate-escode-cli-rust-embedded-search-corpus.mjs",
    "--check",
  ]);
  await run(process.execPath, [
    "--import",
    "tsx",
    "scripts/generate-escode-cli-rust-webfetch-corpus.mjs",
    "--check",
  ]);
  await run(process.execPath, [
    "--import",
    "tsx",
    "scripts/generate-escode-cli-rust-session-context-corpus.mjs",
    "--check",
  ]);
  await run(process.execPath, [
    "--import",
    "tsx",
    "scripts/generate-escode-cli-rust-official-plugins.mjs",
    "--check",
  ]);
  // 修复：这里过去把多个生成脚本串在一次 node 调用里，node 只执行第一个（其余成为 argv），
  // custom-commands 与 memory 的漂移检查实际从未运行。逐个运行。
  for (const script of [
    "scripts/generate-escode-cli-rust-cron-corpus.mjs",
    "scripts/generate-escode-cli-rust-offpeak-corpus.mjs",
    "scripts/generate-escode-cli-rust-custom-commands.mjs",
    "scripts/generate-escode-cli-rust-memory-corpus.mjs",
    "scripts/generate-escode-cli-rust-title-corpus.mjs",
    "scripts/generate-escode-cli-rust-websearch-corpus.mjs",
    "scripts/generate-escode-cli-rust-mcp-oauth-corpus.mjs",
    "scripts/generate-escode-cli-rust-mcp-official-auth-corpus.mjs",
    "scripts/generate-escode-cli-rust-saved-workflow-corpus.mjs",
    "scripts/generate-escode-cli-rust-saved-workflow-store-corpus.mjs",
    "scripts/generate-escode-cli-rust-saved-workflow-tool-corpus.mjs",
    "scripts/generate-escode-cli-rust-dwf-journal-corpus.mjs",
    "scripts/generate-escode-cli-rust-model-catalog-corpus.mjs",
    "scripts/generate-escode-cli-rust-workflow-analysis-corpus.mjs",
    // v3.15.1 对齐（docs/specs/rust-v3.15.1-drift.md）：guarded 危险命令匹配与输入的模型边界材料。
    "scripts/generate-escode-cli-rust-guarded-corpus.mjs",
    "scripts/generate-escode-cli-rust-conversation-input-corpus.mjs",
  ])
    await run(process.execPath, ["--import", "tsx", script, "--check"]);
}
// 测试导入的工作区包（@escode/rpc 等）类型入口指向 dist/*.d.ts；干净检出（CI）没有 dist 时，tsc 会退回按源码
// 以测试的严格配置编译这些包而报错。先按 pnpm typecheck 的同一组项目构建声明。
await run(process.execPath, [
  "node_modules/typescript/bin/tsc",
  "-b",
  "packages/rpc",
  "packages/provider",
  "packages/provider-node",
  "packages/shared",
  "packages/services",
]);
// 测试 tsconfig 类型检查只在第 1 片跑。
if (primary) {
  await run(process.execPath, [
    "node_modules/typescript/bin/tsc",
    "-p",
    "packages/services/tests/tsconfig.escode-cli-rust.json",
  ]);
}
// 首选 cargo-nextest（重试/慢测试超时/JUnit 见 apps/escode-cli-rust/.config/nextest.toml）；
// 本机未安装时回退 cargo test 并给出安装提示（CI 经 taiki-e/install-action 预装）。
// cargo 测试只在第 1 片跑；其余分片只构建下面的二进制供 App 集成套件使用。
if (primary) {
  const hasNextest = spawnSync("cargo", ["nextest", "--version"], { stdio: "ignore" }).status === 0;
  if (hasNextest) {
    await run("cargo", [
      "nextest",
      "run",
      "--locked",
      "--workspace",
      "--manifest-path",
      "apps/escode-cli-rust/Cargo.toml",
      ...(serial ? ["--test-threads=1"] : []),
    ]);
  } else {
    console.warn(
      "[test-escode-cli-rust] cargo-nextest 未安装，回退 cargo test（安装：cargo install cargo-nextest --locked）",
    );
    await run("cargo", [
      "test",
      "--locked",
      "--workspace",
      "--manifest-path",
      "apps/escode-cli-rust/Cargo.toml",
      ...(serial ? ["--", "--test-threads=1"] : []),
    ]);
  }
}
await run("cargo", [
  "build",
  "--examples",
  "--locked",
  "--manifest-path",
  "apps/escode-cli-rust/Cargo.toml",
]);
await run("cargo", ["build", "--locked", "--manifest-path", "apps/escode-cli-rust/Cargo.toml"]);
// Node 与 Rust 的差分用例直接运行 Node CLI 产物；干净检出（CI）没有它时用桌面打包同一脚本构建。
if (!existsSync(resolve(root, "apps/escode-cli/packages/cli/dist/escode.cjs"))) {
  await run(process.execPath, ["scripts/build-desktop-agent-cli.mjs"]);
}
// 工作流分析桥（docs/specs/rust-dynamic-workflow.md 第 3 期）：经 Node CLI 产物的隐藏子命令逐字比对 TS 语料；
// 上面的 workspace 测试缺少启动器时跳过这一条，这里补上启动器单独再跑。
// 工作流分析桥只在第 1 片跑。
if (primary) {
  await run(
    "cargo",
    [
      "test",
      "--locked",
      "--manifest-path",
      "apps/escode-cli-rust/Cargo.toml",
      "-p",
      "escode-cli-tools",
      "workflow_analyzer::tests::bridge_matches_ts_analyzer",
    ],
    {
      ESCODE_PLUGIN_HOST_EXEC_PATH: process.execPath,
      ESCODE_PLUGIN_HOST_ENTRYPOINT: resolve(root, "apps/escode-cli/packages/cli/dist/escode.cjs"),
    },
  );
}
const tests = (await readdir(resolve(root, "packages/services/tests")))
  .filter((name) => /^escode-cli-rust-.*\.test\.ts$/.test(name))
  .sort()
  .filter((_, index) => index % shardCount === shardIndex - 1)
  .map((name) => `packages/services/tests/${name}`);
if (!tests.length) throw new Error("Rust App integration tests are missing");
await run(process.execPath, [
  "--import",
  "tsx",
  "--test",
  ...(serial ? ["--test-concurrency=1"] : []),
  // 单个用例挂死（等不到的事件、遗留子进程）不能拖垮整个作业：5 分钟上限，结束后强制退出，失败详情照常打印。
  "--test-timeout=300000",
  "--test-force-exit",
  ...tests,
]);
