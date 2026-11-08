// 独立 Node 宿主入口（docs/specs/rust-release-rollback.md「Rust-only 打包产物开关」）：
// Rust-only 安装包不含完整 agent bundle（zcode.cjs），但 Rust runtime 仍需要几个隐藏 Node 子命令：
// `__escode-plugin-host`（官方插件 MCP 宿主）、`__escode-workflow-analyzer`、
// `__escode-workflow-host`（动态工作流常驻宿主）与 `__escode-workflow-snippet`。
// 这些分派必须在导入 Agent / 工具注册表之前完成，否则每个 MCP 子进程都会持有整套业务依赖；
// 工作流相关命令沿用 main.ts 的动态 import，只在真正调用时加载。
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
    switch (argv[0]) {
      case "__escode-workflow-analyzer": {
        const { runWorkflowAnalyzerCommand } = await import("./workflow-analyzer-command.js");
        process.exitCode = await runWorkflowAnalyzerCommand();
        return;
      }
      case "__escode-workflow-host": {
        const { runWorkflowHostCommand } = await import("./workflow-host-command.js");
        process.exitCode = await runWorkflowHostCommand();
        return;
      }
      case "__escode-workflow-snippet": {
        const { runWorkflowSnippetCommand } = await import("./workflow-snippet-command.js");
        process.exitCode = await runWorkflowSnippetCommand();
        return;
      }
      default:
        process.exitCode = await runPluginHostCommand(context, argv.slice(1));
    }
  } catch (error) {
    process.stderr.write(`${error instanceof Error ? error.message : String(error)}\n`);
    process.exitCode = 1;
  } finally {
    runtimeWarnings.restore();
  }
}
