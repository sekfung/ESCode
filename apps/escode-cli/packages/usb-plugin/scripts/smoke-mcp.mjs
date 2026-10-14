/**
 * MCP 冒烟：按 stdio 的换行分隔 JSON-RPC 直接拉起 `dist/mcp/server.js`，验证
 * initialize / tools/list / tools/call 三步都通。
 *
 * 为什么需要它：插件是由 **manifest 自声明**的 MCP server，宿主只在会话连接时才拉起它；
 * 一旦注册面出问题（例如协议协商被拒、bundle 里 ESM/CJS 混用导致 import 失败），
 * 表现是「工具整组不存在」而不是报错。这里用最朴素的协议交互把这条路走一遍。
 *
 * 用法：pnpm --filter @escode/usb-plugin build && node apps/escode-cli/packages/usb-plugin/scripts/smoke-mcp.mjs
 */

import { spawn } from "node:child_process";
import { mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";

const packageRoot = resolve(import.meta.dirname, "..");
const entry = resolve(packageRoot, "dist", "mcp", "server.js");
const pluginData = await mkdtemp(join(tmpdir(), "escode-usb-mcp-smoke-"));

const child = spawn(process.execPath, [entry], {
  env: {
    // PATH 指向不存在的目录：探测不到外部抓包工具，正好验证「不可用 + remediation」这条分支。
    PATH: join(dirname(pluginData), "no-such-path"),
    USB_PLUGIN_DATA: pluginData,
    USB_PLUGIN_WORKSPACE: pluginData,
  },
  stdio: ["pipe", "pipe", "pipe"],
});

const responses = [];
let buffer = "";
let stderr = "";
child.stdout.setEncoding("utf8");
child.stdout.on("data", (chunk) => {
  buffer += chunk;
  let index = buffer.indexOf("\n");
  while (index >= 0) {
    const line = buffer.slice(0, index).trim();
    buffer = buffer.slice(index + 1);
    if (line) responses.push(JSON.parse(line));
    index = buffer.indexOf("\n");
  }
});
child.stderr.setEncoding("utf8");
child.stderr.on("data", (chunk) => {
  stderr += chunk;
});

const send = (message) => child.stdin.write(`${JSON.stringify(message)}\n`);
const waitFor = async (id, timeoutMs = 15_000) => {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const found = responses.find((response) => response.id === id);
    if (found) return found;
    await new Promise((resolveWait) => setTimeout(resolveWait, 50));
  }
  throw new Error(`等待响应 ${id} 超时；stderr：${stderr.trim() || "（空）"}`);
};

const failures = [];
const check = (condition, message) => {
  if (!condition) failures.push(message);
};

try {
  send({
    jsonrpc: "2.0",
    id: 1,
    method: "initialize",
    params: {
      protocolVersion: "2025-11-25",
      capabilities: {},
      clientInfo: { name: "usb-plugin-smoke", version: "0.1.0" },
    },
  });
  const init = await waitFor(1);
  check(init.result?.serverInfo?.name === "usb", `initialize 返回了意外结果：${JSON.stringify(init)}`);
  check(typeof init.result?.instructions === "string", "initialize 没有带回 instructions");
  send({ jsonrpc: "2.0", method: "notifications/initialized" });

  send({ jsonrpc: "2.0", id: 2, method: "tools/list" });
  const list = await waitFor(2);
  const tools = list.result?.tools ?? [];
  const names = tools.map((tool) => tool.name).sort();
  check(
    JSON.stringify(names) === JSON.stringify(["usb_capture", "usb_decode", "usb_probe"]),
    `工具清单不符：${JSON.stringify(names)}`,
  );
  const readOnly = tools.filter((tool) => tool.annotations?.readOnlyHint).map((tool) => tool.name).sort();
  check(
    JSON.stringify(readOnly) === JSON.stringify(["usb_decode", "usb_probe"]),
    `只读标注不符：${JSON.stringify(readOnly)}`,
  );
  check(
    tools.find((tool) => tool.name === "usb_capture")?.annotations?.destructiveHint === true,
    "usb_capture 必须标 destructiveHint（要走审批）",
  );

  send({ jsonrpc: "2.0", id: 3, method: "tools/call", params: { name: "usb_probe", arguments: {} } });
  const probe = await waitFor(3);
  const probeText = probe.result?.content?.[0]?.text ?? "";
  check(probeText.startsWith("平台："), `usb_probe 输出异常：${probeText.slice(0, 120)}`);
  check(/抓包：(可用|不可用)/u.test(probeText), "usb_probe 没有给出抓包可用性");

  send({
    jsonrpc: "2.0",
    id: 4,
    method: "tools/call",
    params: { name: "usb_decode", arguments: { source: "/nonexistent-capture.pcapng" } },
  });
  const decode = await waitFor(4);
  check(
    decode.result?.isError === true && /\[notFound\]/u.test(decode.result?.content?.[0]?.text ?? ""),
    "usb_decode 对不存在的文件没有返回 notFound",
  );

  send({ jsonrpc: "2.0", id: 5, method: "tools/call", params: { name: "usb_decode", arguments: { source: "x", bogus: 1 } } });
  const invalid = await waitFor(5);
  check(
    invalid.result?.isError === true && /\[invalidInput\]/u.test(invalid.result?.content?.[0]?.text ?? ""),
    "usb_decode 没有拒绝未知参数",
  );
} catch (error) {
  failures.push(error instanceof Error ? error.message : String(error));
} finally {
  child.stdin.end();
  child.kill("SIGTERM");
}

if (failures.length > 0) {
  console.error("MCP 冒烟失败：");
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exitCode = 1;
} else {
  console.log("MCP 冒烟通过：initialize / tools/list / tools/call（probe、notFound、invalidInput）");
}
