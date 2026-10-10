import { realpath } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { Server } from "@modelcontextprotocol/server";
import { serveStdio } from "@modelcontextprotocol/server/stdio";
import { captureSerialBrokerConnection, createSerialBrokerSend } from "./broker-client.js";
import { SERIAL_TOOLS, handleSerialToolCall } from "./tools.js";

const SERVER_VERSION = "0.1.0";
const SERVER_INSTRUCTIONS =
  "Serial port tools share one serial session with the user's Serial Port panel in this ESCode window. " +
  "Call serial_list first. Never switch away from a port the user has open; ask the user instead. " +
  "When several ports are open, pass path to every tool. " +
  "Use serial_read with the returned lastSeq to follow output, and serial_wait_for to wait for boot logs or responses.";

export function createSerialMcpServer(
  send = createSerialBrokerSend(captureSerialBrokerConnection()),
): Server {
  const server = new Server(
    { name: "serial", version: SERVER_VERSION },
    { capabilities: { tools: {} }, instructions: SERVER_INSTRUCTIONS },
  );
  server.setRequestHandler("tools/list", async () => ({ tools: SERIAL_TOOLS }));
  server.setRequestHandler("tools/call", async (request, extra) => {
    const signal = extra.mcpReq.signal;
    try {
      const result = await handleSerialToolCall({
        name: request.params.name,
        args: request.params.arguments,
        meta: extra.mcpReq._meta,
        send,
        signal,
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

/** 由官方插件宿主（__escode-plugin-host）导入并调用；连接材料必须在 main() 内捕获。 */
export async function main(): Promise<void> {
  process.title = "escode-serial-mcp";
  const send = createSerialBrokerSend(captureSerialBrokerConnection());
  const handle = serveStdio(() => createSerialMcpServer(send), { legacy: "reject" });
  const shutdown = () => {
    void handle
      .close()
      .catch(() => undefined)
      .finally(() => process.exit(0));
  };
  // 宿主关闭 stdin 即表示 MCP 连接结束；输出管道断开也直接退出，避免孤儿进程。
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
      `serial MCP server failed: ${error instanceof Error ? error.stack : String(error)}\n`,
    );
    process.exitCode = 1;
  });
}
