import { existsSync } from "node:fs";
import { dirname, join } from "node:path";
import type { McpServerConfig, PluginLoadOutcome } from "@escode/contracts";
import { createBundledMcpRuntimeConfig } from "./official-plugin-runtime.js";
import { OFFICIAL_REA_PLUGIN_ID } from "./official-plugin-definitions.js";

export const REA_MCP_SERVER_NAME = "rea";

/**
 * REA 的 Ghidra/Hopper 启动、JADX 反编译与浏览器抓取都偏慢，默认 30s 不够；90s 沿用 image-search 先例。
 */
const REA_MCP_TIMEOUT_MS = 90_000;

/** 运行时入口相对运行时根的位置；与插件入口 scripts/../src/server.mjs 的解析保持一致。 */
const RUNTIME_ENTRY_RELATIVE = ["rea-agents", "scripts", "rea.mjs"] as const;

/** 逃生口选项：不映射成同名 env，而是把 JSON 对象的键值直接并入 env。 */
const EXTRA_ENV_OPTION = "extraEnvJson";

/**
 * manifest 的 userConfig 选项键 → REA 的 env 名。不做对照表：按 SCREAMING_SNAKE 机械转换
 * （ghidraInstallDir → GHIDRA_INSTALL_DIR、reaAnalysisProvider → REA_ANALYSIS_PROVIDER），
 * TS 与 Rust 各自实现同一条规则即可，不必维护第二份会漂移的表。
 */
function optionKeyToEnvName(key: string): string {
  return key.replaceAll(/[A-Z]/g, (char) => `_${char}`).toUpperCase();
}

/**
 * 把用户在插件设置里填的可选引擎路径转成 REA 的 env。
 *
 * 这些依赖（Hopper/Ghidra/IDA/JDK/JADX/adb/WABT/tshark/JEB/ILSpy）刻意不随包分发：商业引擎无法
 * 分发，其余是平台与体积问题。配置项只在用户真正填写时才注入，未填就交给 REA 自己探测与报错。
 */
function resolveReaEngineEnv(plugin: {
  configuredOptions?: Record<string, unknown>;
  userConfig?: Record<string, { default?: unknown }>;
}): Record<string, string> {
  const env: Record<string, string> = {};
  const keys = new Set([
    ...Object.keys(plugin.userConfig ?? {}),
    ...Object.keys(plugin.configuredOptions ?? {}),
  ]);
  for (const key of keys) {
    if (key === EXTRA_ENV_OPTION) continue;
    const raw = plugin.configuredOptions?.[key] ?? plugin.userConfig?.[key]?.default;
    if (raw === undefined || raw === null) continue;
    const value = String(raw).trim();
    if (value === "") continue;
    env[optionKeyToEnvName(key)] = value;
  }
  const extra = plugin.configuredOptions?.[EXTRA_ENV_OPTION] ?? plugin.userConfig?.[EXTRA_ENV_OPTION]?.default;
  if (typeof extra === "string" && extra.trim() !== "") {
    try {
      const parsed: unknown = JSON.parse(extra);
      if (parsed !== null && typeof parsed === "object" && !Array.isArray(parsed)) {
        for (const [key, value] of Object.entries(parsed as Record<string, unknown>)) {
          if (typeof value === "string" && value.trim() !== "") env[key] = value.trim();
        }
      }
    } catch {
      // 非法 JSON 只让这一项失效，不阻断 server 启动；表单项的说明里已写明必须是 JSON 对象。
    }
  }
  return env;
}

/**
 * REA 运行时（约 100 MB / 6,555 文件）不能走插件 seed：seed 每次启动都会 walk + sha256 全部插件文件
 * （实测这棵树 915 ms），首次填缓存还要整棵复制，实测会让 bootstrap 的 plugins 测试从 ~3s 变成 5s 超时。
 * 所以它由打包链路单独 stage，这里只负责把路径算出来交给 server。
 *
 * 查找顺序（与 docs/specs/rea-plugin.md 一致）：
 *   1. agent bundle 旁的 rea-runtime/node_modules —— 打包/远端：插件从缓存运行，运行时留在 resources；
 *   2. 插件根 node_modules —— 开发态：插件直接从仓库目录解析，运行时装在这里（见插件包 README）。
 */
function resolveReaRuntimeRoot(pluginRoot: string): string | undefined {
  const candidates: string[] = [];
  const hostEntrypoint = process.env.ESCODE_PLUGIN_HOST_ENTRYPOINT?.trim();
  if (hostEntrypoint) {
    candidates.push(join(dirname(hostEntrypoint), "rea-runtime", "node_modules"));
  }
  candidates.push(join(pluginRoot, "node_modules"));
  return candidates.find((candidate) =>
    existsSync(join(candidate, ...RUNTIME_ENTRY_RELATIVE)),
  );
}

/**
 * rea MCP server 由宿主注入（不在插件 manifest 里声明）：REA 运行时随应用分发，必须用自带 Node
 * 拉起 —— 用户机器上不保证有可用 Node/npm，而打包桌面默认的 Rust 宿主也无法执行 npx 这类 .cmd
 * shim（裸 Command::new 不解析 PATHEXT，也不包 cmd.exe）。
 *
 * manifest 声明同样不可行：seed 会把内置插件 manifest 的 mcpServers 无条件重写成
 * `<root>/dist/mcp/server.js`，那样就拿不到 compact profile 等注入项。
 *
 * 只有 rea 官方插件启用时才注册；插件关闭后模型看不到这组工具，技能也随插件一起消失。
 */
export function resolveBuiltInReaMcpServers(input: {
  pluginOutcome: Pick<PluginLoadOutcome, "plugins">;
  workingDirectory: string;
}): Record<string, McpServerConfig> {
  const plugin = input.pluginOutcome.plugins.find(
    (candidate) => candidate.id === OFFICIAL_REA_PLUGIN_ID && candidate.enabled,
  );
  if (!plugin) return {};
  const runtimeRoot = resolveReaRuntimeRoot(plugin.rootPath);
  const config = createBundledMcpRuntimeConfig({
    cwd: input.workingDirectory,
    env: {
      ESCODE_PLUGIN_ROOT: plugin.rootPath,
      // 诊断落盘目录：入口把 REA 的 stderr tee 到这里并写运行时自检报告
      // （Rust 宿主丢弃子进程 stderr，打包态这是唯一的诊断入口）。
      ESCODE_PLUGIN_DATA: plugin.dataPath,
      // 170 个工具：完整输入 schema 约 425 KB，compact 压到约 253 KB。这是唯一会进模型的 schema，
      // 服务端校验不受影响；被裁掉的引导由 vendored 技能承担。
      REA_MCP_INPUT_SCHEMA_PROFILE: "compact",
      // 可选增强（Ghidra/Hopper/IDA/JDK/WABT/tshark/JEB/ILSpy…）的路径来自插件 userConfig。
      ...resolveReaEngineEnv(plugin),
      // 运行时在插件缓存之外时由这里指路；缺失不阻断注册 —— 让用户在 MCP 状态里看到失败，
      // 好过工具静默消失（入口脚本会给出可读原因）。
      ...(runtimeRoot ? { REA_RUNTIME_ROOT: runtimeRoot } : {}),
    },
    rootPath: plugin.rootPath,
    timeoutMs: REA_MCP_TIMEOUT_MS,
  });
  if (!config) return {};
  // 不设 isolation：插件/host 注入之外没有该键的入口，而默认就是 session —— REA 在服务端持有
  // binary session，按会话独占一个进程，会话结束即可回收它拉起的分析引擎。
  return { [REA_MCP_SERVER_NAME]: config };
}
