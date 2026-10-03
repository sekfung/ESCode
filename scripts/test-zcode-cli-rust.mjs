import { spawn } from "node:child_process";
import { existsSync } from "node:fs";
import { readdir } from "node:fs/promises";
import { resolve } from "node:path";

const root = resolve(import.meta.dirname, "..");
// CI 与慢机器：每个用例都会起 runtime + 本地模型服务，并发跑会被接收超时误伤；ZCODE_TEST_SERIAL=1 时串行。
const serial = process.env.ZCODE_TEST_SERIAL === "1";
async function run(command, args, env = {}) {
  await new Promise((done, fail) => {
    const child = spawn(command, args, {
      cwd: root,
      stdio: "inherit",
      env: {
        ...process.env,
        TSX_TSCONFIG_PATH: resolve(root, "packages/services/tests/tsconfig.zcode-cli-rust.json"),
        ...env,
      },
    });
    child.once("error", fail);
    child.once("close", (code, signal) =>
      code === 0 ? done() : fail(new Error(`${command} failed: ${signal ?? code}`)),
    );
  });
}
await run(process.execPath, ["apps/zcode-cli/packages/dynamic-workflow/scripts/generate-libs.mjs"]);
// 差分与导入用例直接 import TS bootstrap 源码，其依赖包走 dist 入口；干净检出时必须先构建，
// 否则只报 ERR_MODULE_NOT_FOUND。经 npm_execpath 调 pnpm，避免 Windows 上 pnpm.cmd 需要 shell。
if (!process.env.npm_execpath) throw new Error("Run via `pnpm test:zcode-cli-rust`");
await run(process.execPath, [
  process.env.npm_execpath,
  "--dir",
  "apps/zcode-cli",
  "--filter",
  "@zcode/bootstrap^...",
  "build",
]);
await run(process.execPath, [
  "--import",
  "tsx",
  "scripts/generate-zcode-cli-rust-prompt.mjs",
  "--check",
]);
await run(process.execPath, [
  "--import",
  "tsx",
  "scripts/generate-zcode-cli-rust-tool-schemas.mjs",
  "--check",
]);
await run(process.execPath, [
  "--import",
  "tsx",
  "scripts/generate-zcode-cli-rust-permission-matrix.mjs",
  "--check",
]);
await run(process.execPath, [
  "--import",
  "tsx",
  "scripts/generate-zcode-cli-rust-rewind-branch-corpus.mjs",
  "--check",
]);
await run(process.execPath, [
  "--import",
  "tsx",
  "scripts/generate-zcode-cli-rust-bash-readonly-corpus.mjs",
  "--check",
]);
await run(process.execPath, [
  "--import",
  "tsx",
  "scripts/generate-zcode-cli-rust-bash-policies.mjs",
  "--check",
]);
await run(process.execPath, [
  "--import",
  "tsx",
  "scripts/generate-zcode-cli-rust-bash-rule-corpus.mjs",
  "--check",
]);
await run(process.execPath, [
  "--import",
  "tsx",
  "scripts/generate-zcode-cli-rust-git-safety-corpus.mjs",
  "--check",
]);
await run(process.execPath, [
  "--import",
  "tsx",
  "scripts/generate-zcode-cli-rust-proxy-corpus.mjs",
  "--check",
]);
await run(process.execPath, [
  "--import",
  "tsx",
  "scripts/generate-zcode-cli-rust-init-prompt-corpus.mjs",
  "--check",
]);
await run(process.execPath, [
  "--import",
  "tsx",
  "scripts/generate-zcode-cli-rust-embedded-search-corpus.mjs",
  "--check",
]);
await run(process.execPath, [
  "--import",
  "tsx",
  "scripts/generate-zcode-cli-rust-webfetch-corpus.mjs",
  "--check",
]);
await run(process.execPath, [
  "--import",
  "tsx",
  "scripts/generate-zcode-cli-rust-session-context-corpus.mjs",
  "--check",
]);
await run(process.execPath, [
  "--import",
  "tsx",
  "scripts/generate-zcode-cli-rust-official-plugins.mjs",
  "--check",
]);
// 修复：这里过去把多个生成脚本串在一次 node 调用里，node 只执行第一个（其余成为 argv），
// custom-commands 与 memory 的漂移检查实际从未运行。逐个运行。
for (const script of [
  "scripts/generate-zcode-cli-rust-cron-corpus.mjs",
  "scripts/generate-zcode-cli-rust-offpeak-corpus.mjs",
  "scripts/generate-zcode-cli-rust-custom-commands.mjs",
  "scripts/generate-zcode-cli-rust-memory-corpus.mjs",
  "scripts/generate-zcode-cli-rust-title-corpus.mjs",
  "scripts/generate-zcode-cli-rust-websearch-corpus.mjs",
  "scripts/generate-zcode-cli-rust-mcp-oauth-corpus.mjs",
  "scripts/generate-zcode-cli-rust-mcp-official-auth-corpus.mjs",
  "scripts/generate-zcode-cli-rust-saved-workflow-corpus.mjs",
  "scripts/generate-zcode-cli-rust-saved-workflow-store-corpus.mjs",
  "scripts/generate-zcode-cli-rust-saved-workflow-tool-corpus.mjs",
  "scripts/generate-zcode-cli-rust-dwf-journal-corpus.mjs",
  "scripts/generate-zcode-cli-rust-model-catalog-corpus.mjs",
  "scripts/generate-zcode-cli-rust-workflow-analysis-corpus.mjs",
])
  await run(process.execPath, ["--import", "tsx", script, "--check"]);
// 测试导入的工作区包（@zcode/rpc 等）类型入口指向 dist/*.d.ts；干净检出（CI）没有 dist 时，tsc 会退回按源码
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
await run(process.execPath, [
  "node_modules/typescript/bin/tsc",
  "-p",
  "packages/services/tests/tsconfig.zcode-cli-rust.json",
]);
await run("cargo", [
  "test",
  "--locked",
  "--workspace",
  "--manifest-path",
  "apps/zcode-cli-rust/Cargo.toml",
  ...(serial ? ["--", "--test-threads=1"] : []),
]);
await run("cargo", [
  "build",
  "--examples",
  "--locked",
  "--manifest-path",
  "apps/zcode-cli-rust/Cargo.toml",
]);
await run("cargo", ["build", "--locked", "--manifest-path", "apps/zcode-cli-rust/Cargo.toml"]);
// Node 与 Rust 的差分用例直接运行 Node CLI 产物；干净检出（CI）没有它时用桌面打包同一脚本构建。
if (!existsSync(resolve(root, "apps/zcode-cli/packages/cli/dist/zcode.cjs"))) {
  await run(process.execPath, ["scripts/build-desktop-agent-cli.mjs"]);
}
// 工作流分析桥（docs/specs/rust-dynamic-workflow.md 第 3 期）：经 Node CLI 产物的隐藏子命令逐字比对 TS 语料；
// 上面的 workspace 测试缺少启动器时跳过这一条，这里补上启动器单独再跑。
await run(
  "cargo",
  [
    "test",
    "--locked",
    "--manifest-path",
    "apps/zcode-cli-rust/Cargo.toml",
    "-p",
    "zcode-cli-tools",
    "workflow_analyzer::tests::bridge_matches_ts_analyzer",
  ],
  {
    ZCODE_PLUGIN_HOST_EXEC_PATH: process.execPath,
    ZCODE_PLUGIN_HOST_ENTRYPOINT: resolve(root, "apps/zcode-cli/packages/cli/dist/zcode.cjs"),
  },
);
const tests = (await readdir(resolve(root, "packages/services/tests")))
  .filter((name) => /^zcode-cli-rust-.*\.test\.ts$/.test(name))
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
