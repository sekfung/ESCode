// 独立插件宿主入口（docs/specs/rust-release-rollback.md「Rust-only 打包产物开关」）：
// Rust-only 安装包不含完整 agent bundle（zcode.cjs），但官方插件仍需 Electron-as-Node 执行
// `__escode-plugin-host`。这里只加载插件宿主必需的模块，必须在导入 Agent / 工具注册表之前分派，
// 否则每个 MCP 子进程都会持有整套业务依赖。
import type { RunContext } from "@escode/shared-types";
import { applyCliRuntimeEnvSanitization } from "./env.js";
import { runPluginHostCommand } from "./plugin-host-command.js";
import { interceptKnownRuntimeWarnings } from "./runtime-warnings.js";
import { ensureSeaRuntimeTools } from "./sea-runtime-tools.js";

void main();

async function main(): Promise<void> {
  const argv = process.argv.slice(2);
  applyCliRuntimeEnvSanitization(process.env);
  const runtimeWarnings = interceptKnownRuntimeWarnings(process.stderr);
  try {
    Object.assign(process.env, await ensureSeaRuntimeTools());
    const context: RunContext = {
      argv,
      stdin: process.stdin,
      stdout: process.stdout,
      stderr: process.stderr,
    };
    process.exitCode = await runPluginHostCommand(context, argv.slice(1));
  } catch (error) {
    process.stderr.write(`${error instanceof Error ? error.message : String(error)}\n`);
    process.exitCode = 1;
  } finally {
    runtimeWarnings.restore();
  }
}
