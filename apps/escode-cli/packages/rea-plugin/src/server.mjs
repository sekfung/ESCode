// 宿主启动器入口的源文件。构建期由 scripts/build-mcp.mjs 产出到 dist/mcp/server.js
// （dist/ 与 circuit/serial 一样是构建产物、不入库，宿主启动器约定读 <root>/dist/mcp/server.js）。
//
// 插件不声明 mcpServers：server 由宿主注入（bootstrap/src/app/built-in-rea.ts 与 Rust mcp_rea.rs），
// 宿主用自带 Node 执行本入口（ELECTRON_RUN_AS_NODE=1），再把 vendored REA 的 CLI 入口按 mcp 模式拉起。
//
// 走 REA 自己的 CLI 入口而不是直接 import dist/main.js：CLI 入口负责设置 REA_PACKAGE_VERSION
// 等启动前置，重复它们只会与上游漂移。
//
// 诊断：Rust 宿主把子进程 stderr 丢进 Stdio::null()，TS 宿主也只把 stderr 尾巴写进脱敏日志，
// 两种形态下 REA 自己的报错都看不到。所以这里主动把 stderr tee 到插件数据目录，并在启动时写一份
// 运行时自检报告（docs/specs/rea-plugin.md 的 P3）。
import { appendFileSync, existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const pluginRoot = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
const RUNTIME_ENTRY_RELATIVE = ["rea-agents", "scripts", "rea.mjs"];

/** 与 userConfig 的选项一一对应（选项键 → env 名是机械转换），这里只报告是否就位，不猜能力。 */
const OPTIONAL_ENGINE_ENV = [
  "GHIDRA_INSTALL_DIR",
  "JAVA_HOME",
  "HOPPER_LAUNCHER_PATH",
  "REA_ANALYSIS_PROVIDER",
  "REA_BROWSER_EXECUTABLE",
  "REA_GHIDRA_STARTUP_TIMEOUT_MS",
  "REA_IDA_MCP_CONFIG",
  "REA_ILSPY_CMD_PATH",
  "REA_JEB_MCP_URL",
  "REA_TSHARK_COMMAND",
  "REA_WABT_BIN_DIRECTORY",
];

/**
 * 运行时解析顺序与宿主（built-in-rea.ts / mcp_rea.rs）保持一致：
 *   1. REA_RUNTIME_ROOT —— 打包态：运行时在 agent bundle 旁（插件从缓存运行）；
 *   2. <pluginRoot>/node_modules —— 开发态：插件直接从仓库目录解析。
 */
function resolveRuntimeEntry() {
  const candidates = [];
  const injected = process.env.REA_RUNTIME_ROOT?.trim();
  if (injected) candidates.push(resolve(injected, ...RUNTIME_ENTRY_RELATIVE));
  candidates.push(join(pluginRoot, "node_modules", ...RUNTIME_ENTRY_RELATIVE));
  return candidates.find((candidate) => existsSync(candidate));
}

function resolveRuntimeRoot(entry) {
  // <root>/node_modules/rea-agents/scripts/rea.mjs → dirname 是 scripts，退两级到 <root>/node_modules。
  return resolve(dirname(entry), "..", "..");
}

function prepareDiagnostics(entry) {
  const dataDir = process.env.ESCODE_PLUGIN_DATA?.trim();
  if (!dataDir) return { logPath: undefined };
  try {
    mkdirSync(dataDir, { recursive: true });
  } catch {
    return { logPath: undefined };
  }
  const logPath = join(dataDir, "rea-mcp.log");
  const runtimeRoot = resolveRuntimeRoot(entry);
  let reaVersion = "(unknown)";
  try {
    reaVersion = JSON.parse(readFileSync(join(runtimeRoot, "rea-agents", "package.json"), "utf8")).version;
  } catch {
    // 版本读不到不影响启动；报告里保持 (unknown)。
  }
  const lines = [
    `time: ${new Date().toISOString()}`,
    `reaAgentsVersion: ${reaVersion}`,
    `runtimeRoot: ${runtimeRoot}`,
    `pluginRoot: ${pluginRoot}`,
    `node: ${process.version} (${process.platform}-${process.arch})`,
    `nodeMode: ${process.env.ELECTRON_RUN_AS_NODE === "1" ? "electron-as-node" : "plain-node"}`,
    `schemaProfile: ${process.env.REA_MCP_INPUT_SCHEMA_PROFILE ?? "(default)"}`,
    "optional engines (env presence):",
    ...OPTIONAL_ENGINE_ENV.map((key) => `  ${key}: ${process.env[key]?.trim() ? "set" : "-"}`),
    "",
    "REA 自己报告的缺依赖/平台限制会写在同目录的 rea-mcp.log；",
    "Rust 宿主丢弃子进程 stderr，所以这个文件是打包态唯一的诊断入口。",
    "",
  ];
  try {
    writeFileSync(join(dataDir, "rea-runtime-report.txt"), lines.join("\n"));
    // 先写一行会话分隔，保证日志文件一定存在 —— 否则 REA 不写 stderr 时用户会找不到"诊断入口"。
    appendFileSync(logPath, `--- rea mcp session start ${new Date().toISOString()} ---\n`);
  } catch {
    // 报告写不进去不影响启动。
  }
  return { logPath };
}

/** stderr 同时透传（TS 宿主会脱敏落日志）与落盘（Rust 宿主丢弃 stderr 时的唯一线索）。 */
function teeStderr(logPath) {
  if (!logPath) return;
  const original = process.stderr.write.bind(process.stderr);
  const maxBytes = 1_000_000;
  let written = 0;
  process.stderr.write = (chunk, encoding, callback) => {
    try {
      if (written < maxBytes) {
        const text = typeof chunk === "string" ? chunk : Buffer.from(chunk).toString("utf8");
        appendFileSync(logPath, text);
        written += text.length;
      }
    } catch {
      // 落盘失败不能吃掉原始输出：继续走 original。
    }
    return original(chunk, encoding, callback);
  };
}

// argv 重排与诊断必须在 import REA 之前完成：REA 的 CLI 入口按 argv 判定是否进入 MCP 模式
// （scripts/rea.mjs: args.length===1 && "mcp"），启动器传进来的 "__escode-plugin-host <script>" 不能留在里面。
const entry = resolveRuntimeEntry();
if (!entry) {
  process.stderr.write(
    "[rea] 找不到 REA 运行时（REA_RUNTIME_ROOT 与插件根 node_modules 都没有）。" +
      "打包态说明 agent 包缺少 rea-runtime；开发态请先运行 " +
      "`node scripts/install-rea-runtime.mjs`（插件包内）。\n",
  );
  process.exit(1);
}

const { logPath } = prepareDiagnostics(entry);
teeStderr(logPath);

process.argv = [process.argv[0] ?? "node", entry, "mcp"];
await import(pathToFileURL(entry).href);
