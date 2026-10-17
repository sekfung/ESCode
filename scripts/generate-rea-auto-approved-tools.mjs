// 从已安装的 REA 生成「免审批工具」清单（defaultAllowedTools）。
//
// 为什么是生成器而不是手写清单：清单必须与固定的 REA 版本一致，而 REA 的工具面有 170 个且随版本变化。
// 手抄到 TS 与 Rust 两边必然漂移，所以这里一次生成、两边引用：
//   - TS：apps/escode-cli/packages/bootstrap/src/app/rea-auto-approved-tools.ts（definition 直接引用）
//   - Rust：随 scripts/generate-escode-cli-rust-official-plugins.mjs 写进 official_plugins.json
//
// 分类边界是「有没有副作用」，不是按数量挑：
//   放行 = REA 自标 readOnlyHint，或命中读语义前缀且不在例外表里，或点名列入只读分析工具；
//   其余（写、执行、运行时附加、驱动 GUI、网络与浏览器、会话/引擎启动、破坏性）一律保持逐次审批。
//
// 只在升级 REA 版本时人工运行：node scripts/generate-rea-auto-approved-tools.mjs
import { spawn } from "node:child_process";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const pluginRoot = join(repoRoot, "apps", "escode-cli", "packages", "rea-plugin");
const pinnedVersion = JSON.parse(
  readFileSync(join(pluginRoot, "package.json"), "utf8"),
)?.escodePlugin?.reaAgentsVersion?.trim();
if (!pinnedVersion) throw new Error("插件 package.json 缺少 escodePlugin.reaAgentsVersion");

const entry = join(pluginRoot, "dist", "mcp", "server.js");
const outputPath = join(
  repoRoot,
  "apps",
  "escode-cli",
  "packages",
  "bootstrap",
  "src",
  "app",
  "rea-auto-approved-tools.ts",
);

/** 读语义前缀：命中即放行（除非在例外表里）。 */
export const AUTO_APPROVED_PREFIXES = [
  "address_",
  "build_",
  "compare_",
  "current_",
  "decode_",
  "get_",
  "inspect_",
  "list_",
  "procedure_",
  "project_",
  "read_",
  "resolve_",
  "search_",
  "verify_",
  "xrefs",
];

/** 同前缀但会启动浏览器 / 加载远端内容，必须保持审批。 */
export const AUTO_APPROVED_EXCEPTIONS = ["inspect_web_page", "inspect_electron_page"];

/** 逐个点名的只读分析工具（反编译读取类，不含跑目标进程或驱动界面的动作）。 */
export const AUTO_APPROVED_EXACT = [
  "analyze_function",
  "analyze_javascript_application",
  "analyze_swift_types",
  "batch_decompile",
  "binary_overview",
  "comment",
  "correlate_static_and_runtime",
  "decompile_jeb_item",
  "demangle_swift",
  "find_changed_behavior",
  "find_xrefs_to_name",
  "inline_comment",
];

/** 与上面规则同一份判据，供生成器与测试共用（测试用它断言清单没被手改宽）。 */
export function isAutoApproved(tool) {
  if (tool.annotations?.readOnlyHint === true) return true;
  if (tool.annotations?.destructiveHint === true) return false;
  if (AUTO_APPROVED_EXCEPTIONS.includes(tool.name)) return false;
  if (AUTO_APPROVED_EXACT.includes(tool.name)) return true;
  return AUTO_APPROVED_PREFIXES.some((prefix) => tool.name.startsWith(prefix));
}

/**
 * 必须保持逐次审批的前缀（写、执行、运行时附加、驱动界面、设备与网络、会话/引擎启动）。
 * 与 AUTO_APPROVED_PREFIXES 是同一道边界的另一侧：测试用它对清单做反向断言，
 * 防止有人手改生成物把副作用工具混进免审批面。
 */
export const GATED_PREFIXES = [
  "annotate_",
  "capture_",
  "collect_",
  "discover_",
  "export_",
  "extract_",
  "goto_",
  "import_",
  "install_",
  "next_",
  "observe_",
  "open_",
  "prev_",
  "pull_",
  "push_",
  "reconcile_",
  "record_",
  "recover_",
  "set_",
  "start_",
  "stop_",
  "trace_",
  "uninstall_",
  "update_",
];

export function isGatedByPrefix(name) {
  return GATED_PREFIXES.some((prefix) => name.startsWith(prefix));
}

async function listTools() {
  const child = spawn("node", [entry], { stdio: ["pipe", "pipe", "inherit"] });
  let buffer = "";
  const pending = new Map();
  child.stdout.on("data", (chunk) => {
    buffer += String(chunk);
    for (let index = buffer.indexOf("\n"); index >= 0; index = buffer.indexOf("\n")) {
      const line = buffer.slice(0, index);
      buffer = buffer.slice(index + 1);
      if (line.trim() === "") continue;
      let message;
      try {
        message = JSON.parse(line);
      } catch {
        continue;
      }
      const resolvePending = pending.get(message.id);
      if (resolvePending) {
        pending.delete(message.id);
        resolvePending(message);
      }
    }
  });
  let nextId = 1;
  const call = (method, params) =>
    new Promise((resolveCall, reject) => {
      const id = nextId++;
      pending.set(id, resolveCall);
      child.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", id, method, params })}\n`);
      setTimeout(() => reject(new Error(`${method} timeout`)), 120_000);
    });
  try {
    const initialized = await call("initialize", {
      capabilities: {},
      clientInfo: { name: "escode-rea-auto-approved", version: "0" },
      protocolVersion: "2025-06-18",
    });
    child.stdin.write(
      `${JSON.stringify({ jsonrpc: "2.0", method: "notifications/initialized", params: {} })}\n`,
    );
    const listed = await call("tools/list", {});
    return {
      serverVersion: initialized.result?.serverInfo?.version,
      tools: listed.result?.tools ?? [],
    };
  } finally {
    child.stdin.end();
    child.kill();
  }
}

async function main() {
  const { serverVersion, tools } = await listTools();
  if (serverVersion !== pinnedVersion) {
    throw new Error(`REA 版本不符：已安装 ${serverVersion}，pin 是 ${pinnedVersion}`);
  }
  const approved = tools
    .filter(isAutoApproved)
    .map((tool) => tool.name)
    .sort();

  mkdirSync(dirname(outputPath), { recursive: true });
  writeFileSync(
    outputPath,
    `// 生成物：不要手改。由 scripts/generate-rea-auto-approved-tools.mjs 从已安装的 REA 生成，
// 规则见该脚本（读语义前缀 + 例外表 + 点名清单 + REA 自标的 readOnlyHint）。
// 版本变化后必须重新生成，否则 bootstrap/tests/rea-plugin.test.ts 会因版本不一致失败。

/** 生成这份清单时对应的 REA 版本。 */
export const REA_AUTO_APPROVED_TOOLS_VERSION = ${JSON.stringify(serverVersion)};

/** definition 的 defaultAllowedTools：免审批但不会隐藏其它工具（权限放行清单，不是注册面过滤）。 */
export const REA_AUTO_APPROVED_TOOLS: readonly string[] = [
${approved.map((name) => `  ${JSON.stringify(name)},`).join("\n")}
];
`,
  );
  process.stdout.write(
    `rea auto-approved tools: ${approved.length}/${tools.length} -> ${outputPath}\n` +
      `version ${serverVersion}\n`,
  );
}

// 只在直接运行时生成：bootstrap 的测试会 import 上面的规则函数来断言"清单里每个名字都符合规则"，
// 那种情况下不能触发探测与写盘。
if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  await main();
}
