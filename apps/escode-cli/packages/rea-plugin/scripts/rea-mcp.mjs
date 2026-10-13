#!/usr/bin/env node
// REA MCP 启动器（docs/specs/rea-plugin.md）。存在的唯一理由是抹平平台差异：
// 打包桌面默认的 Rust 宿主用裸 Command::new 启动 MCP server，不解析 PATHEXT、不包 cmd.exe，
// 无法直接执行 npx 这类 .cmd shim；而 Windows 上把 cmd.exe 写进 .mcp.json 又会让 macOS/Linux 失效，
// 且桌面 UI 下发 mcpServers 时会剥离 cmd 包装。所以把差异收在这一个脚本里，.mcp.json 保持平台中立。
//
// 本脚本是字节透明的转发者：不解析消息、不重排、不改写。stdout 是 JSON-RPC 通道，任何诊断只走 stderr。
import { spawn } from "node:child_process";
import { readFile } from "node:fs/promises";

const PACKAGE_JSON_URL = new URL("../package.json", import.meta.url);

// REA 的 engines：^22.19.0 || ^24.11.0 || >=26.0.0（23 与 25 被上游排除）。
function satisfiesReaEngines(version) {
  const [major, minor] = version.split(".").map((part) => Number.parseInt(part, 10));
  if (!Number.isInteger(major) || !Number.isInteger(minor)) return false;
  if (major === 22) return minor >= 19;
  if (major === 24) return minor >= 11;
  return major >= 26;
}

async function readPinnedVersion() {
  let parsed;
  try {
    parsed = JSON.parse(await readFile(PACKAGE_JSON_URL, "utf8"));
  } catch (error) {
    throw new Error(`无法读取插件 package.json：${error.message}`);
  }
  const version = parsed?.escodePlugin?.reaAgentsVersion;
  if (typeof version !== "string" || version.trim() === "") {
    throw new Error("插件 package.json 缺少 escodePlugin.reaAgentsVersion");
  }
  return version.trim();
}

function fail(message) {
  process.stderr.write(`[rea] ${message}\n`);
  process.exit(1);
}

const pinnedVersion = await readPinnedVersion().catch((error) => fail(error.message));

if (!satisfiesReaEngines(process.versions.node)) {
  fail(
    `REA 需要 Node ^22.19.0 || ^24.11.0 || >=26.0.0，当前为 ${process.versions.node}。` +
      "请切换到受支持的 Node 版本后重试。",
  );
}

// 产物只要装到用户机器上即可，用 npx 按固定版本拉起，ESCode 不分发 REA 本体。
const specifier = `rea-agents@${pinnedVersion}`;
const npxArgs = ["-y", specifier, "mcp"];
const child =
  process.platform === "win32"
    ? // npx 在 Windows 上是 .cmd shim，Node 无法直接 spawn（ENOENT / EINVAL），必须经 cmd.exe。
      spawn(process.env.ComSpec ?? "cmd.exe", ["/d", "/s", "/c", "npx", ...npxArgs], {
        env: process.env,
        stdio: "inherit",
      })
    : spawn("npx", npxArgs, { env: process.env, stdio: "inherit" });

child.on("error", (error) => {
  fail(
    `无法启动 npx（${error.code ?? error.message}）。` +
      "REA 需要本机 Node 与 npm；请确认它们已安装并在 PATH 中。",
  );
});

child.on("exit", (code, signal) => {
  // 退出码原样传播，让宿主的失败分类与 stderr 诊断看到真实原因。
  process.exit(code ?? (signal ? 1 : 0));
});
