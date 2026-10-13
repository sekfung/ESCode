// 插件构建入口：装运行时（按目标平台）+ 产出宿主启动器入口。
//
// 打包链路经 packages/desktop/scripts/prepare-agent-node-bundle.mjs 的 requiresRuntime 调到这里
// （bootstrap 形态则走 runtimeBuildScript）；开发态需要时手工跑一次，幂等。
// 两步分开保留各自的可读失败信息：安装失败（网络/平台）与入口缺失是两类不同问题。
import { execFileSync } from "node:child_process";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const pluginRoot = join(dirname(fileURLToPath(import.meta.url)), "..");
for (const script of ["install-rea-runtime.mjs", "build-mcp.mjs"]) {
  execFileSync(process.execPath, [join(pluginRoot, "scripts", script)], { stdio: "inherit" });
}
