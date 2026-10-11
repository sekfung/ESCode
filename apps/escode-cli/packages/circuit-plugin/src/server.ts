import { realpath } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { Server } from "@modelcontextprotocol/server";
import { serveStdio } from "@modelcontextprotocol/server/stdio";
import { captureCircuitBrokerConnection, createCircuitBrokerSend } from "./broker-client.js";
import { CIRCUIT_TOOLS, handleCircuitToolCall } from "./tools.js";

const SERVER_VERSION = "0.1.0";
const SERVER_INSTRUCTIONS =
  "Circuit tools read the Circuit IR that the ESCode Host owns for this workspace. " +
  "Call circuit_parse first (then circuit_get) before answering anything about a schematic or PCB, " +
  "and use circuit_query for details instead of guessing. Cite reference designators (R1, U3) and net names " +
  "when you explain or conclude anything, and say which conclusions depend on low-confidence elements. " +
  "For an image or PDF there is no deterministic parse: read the drawing, tell the user what you are about to send, " +
  "then submit the draft with circuit_submit_recognition — it assigns element ids and confidence, and for vector PDFs " +
  "it re-derives the confidence from the PDF's own text and line segments. " +
  "circuit_check and circuit_compare_firmware are read-only: report their deterministic findings separately " +
  "from your own suggestions, and pass nextCursor back as cursor when a result is paged. " +
  "circuit_correct changes the IR the user sees in the circuit panel; circuit_export needs the user's approval. " +
  "If a tool answers 'unavailable', tell the user the circuit tools need ESCode Desktop.";

export function createCircuitMcpServer(
  send = createCircuitBrokerSend(captureCircuitBrokerConnection()),
): Server {
  const server = new Server(
    { name: "circuit", version: SERVER_VERSION },
    { capabilities: { tools: {} }, instructions: SERVER_INSTRUCTIONS },
  );
  server.setRequestHandler("tools/list", async () => ({ tools: CIRCUIT_TOOLS }));
  server.setRequestHandler("tools/call", async (request, extra) => {
    const signal = extra.mcpReq.signal;
    try {
      const result = await handleCircuitToolCall({
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
            text: `[io_error] ${error instanceof Error ? error.message : String(error)}`,
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
  process.title = "escode-circuit-mcp";
  const send = createCircuitBrokerSend(captureCircuitBrokerConnection());
  const handle = serveStdio(() => createCircuitMcpServer(send), { legacy: "reject" });
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
      `circuit MCP server failed: ${error instanceof Error ? error.stack : String(error)}\n`,
    );
    process.exitCode = 1;
  });
}
