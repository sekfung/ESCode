/**
 * usb MCP server（stdio）。由官方插件宿主（`__escode-plugin-host`）按 manifest 的 `mcpServers` 启动。
 *
 * 与 serial 插件的差别：没有 broker、没有反向请求——抓包与解码都在本进程内完成，产物是文件。
 * 因此这里不校验会话上下文，也不依赖 Host；退出时唯一要保证的是杀掉还在跑的抓包子进程。
 */

import { realpath } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { Server } from "@modelcontextprotocol/server";
import { serveStdio } from "@modelcontextprotocol/server/stdio";
import { handleUsbToolCall, USB_TOOLS, type UsbToolContext } from "./tools.js";

const SERVER_VERSION = "0.1.0";
const SERVER_INSTRUCTIONS =
  "USB tools work in three steps: usb_probe (what this machine can capture, which devices are attached, which capture " +
  "files already exist) → usb_capture (record live, needs the user's approval) → usb_decode (protocol-level analysis of a " +
  "capture file). Always call usb_probe before capturing, and never try usb_capture when it reports no backend — report its " +
  "remediation to the user instead. usb_decode is offline and available everywhere, including macOS and for files captured on " +
  "another machine. Start with view=summary, page with view=timeline, then view=message for a single message's bytes. " +
  "State findings in protocol terms (APDU and SW1SW2, CDB/CSW/sense, HID fields), not raw hex.";

export type UsbBaseContext = Omit<UsbToolContext, "signal">;

/** 工作区与插件数据目录由 manifest 注入；缺失时退到 cwd 与临时目录，保证独立运行时也能用。 */
export function resolveUsbBaseContext(env: Record<string, string | undefined>): UsbBaseContext {
  const workspace = env.USB_PLUGIN_WORKSPACE?.trim() || env.ESCODE_PROJECT_DIR?.trim() || process.cwd();
  const pluginData =
    env.USB_PLUGIN_DATA?.trim() || env.ESCODE_PLUGIN_DATA?.trim() || join(tmpdir(), "escode-usb-plugin");
  return {
    env,
    workspace: resolve(workspace),
    pluginData: resolve(pluginData),
  };
}

export function createUsbMcpServer(base: UsbBaseContext): Server {
  const server = new Server(
    { name: "usb", version: SERVER_VERSION },
    { capabilities: { tools: {} }, instructions: SERVER_INSTRUCTIONS },
  );
  server.setRequestHandler("tools/list", async () => ({ tools: USB_TOOLS }));
  server.setRequestHandler("tools/call", async (request, extra) => {
    const signal = extra.mcpReq.signal;
    try {
      const result = await handleUsbToolCall({
        name: request.params.name,
        args: request.params.arguments,
        context: { ...base, signal },
      });
      return { ...result };
    } catch (error) {
      if (signal.aborted) throw error;
      return {
        content: [
          {
            type: "text" as const,
            text: `[io] ${error instanceof Error ? error.message : String(error)}`,
          },
        ],
        isError: true,
      };
    }
  });
  return server;
}

/** 由插件宿主导入并调用。进程结束前必须杀掉在跑的抓包工具，避免留下孤儿进程与 .partial 文件。 */
export async function main(): Promise<void> {
  process.title = "escode-usb-mcp";
  // 不传 `{ legacy: "reject" }`：serial/circuit 能拒绝旧版协商，是因为宿主注入的 MCP 配置里
  // 固定了 protocolVersion（见 built-in-serial.ts）；本插件的 server 由 manifest 自声明，
  // 没有那个注入点，拒绝旧版会让 usb_* 工具整组不可用（与 android-emulator / ios-simulator 同）。
  const handle = serveStdio(() => createUsbMcpServer(resolveUsbBaseContext(process.env)));
  const shutdown = () => {
    void handle
      .close()
      .catch(() => undefined)
      .finally(() => process.exit(0));
  };
  process.stdin.once("end", shutdown);
  process.stdout.once("error", shutdown);
}

async function isDirectEntrypoint(moduleUrl: string, argv1: string | undefined): Promise<boolean> {
  if (!argv1) return false;
  try {
    return (await realpath(fileURLToPath(moduleUrl))) === (await realpath(argv1));
  } catch {
    return false;
  }
}

if (await isDirectEntrypoint(import.meta.url, process.argv[1])) {
  void main().catch((error) => {
    process.stderr.write(
      `usb MCP server failed: ${error instanceof Error ? error.stack : String(error)}\n`,
    );
    process.exitCode = 1;
  });
}
