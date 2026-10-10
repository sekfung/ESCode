import {
  serialToolArgsSchemas,
  type SerialBrokerRequest,
  type SerialToolOp,
} from "@escode/shared/serial";
import { z } from "zod";

/** MCP 工具元数据：readOnlyHint 让只读工具在 plan 模式可用；写类工具标 destructive，plan 模式不放行。 */
export interface SerialToolDefinition {
  name: string;
  description: string;
  inputSchema: { type: "object"; [key: string]: unknown };
  annotations: { readOnlyHint?: boolean; destructiveHint?: boolean; title: string };
}

const TOOL_OPS: Record<string, SerialToolOp> = {
  serial_list: "list",
  serial_open: "open",
  serial_write: "write",
  serial_read: "read",
  serial_wait_for: "waitFor",
  serial_close: "close",
  serial_set_signals: "setSignals",
};

/** 多串口时的通用说明，附在需要定位串口的工具描述后。 */
const PATH_HINT =
  " Pass path when more than one serial port is open; it may be omitted when exactly one is open.";

const DESCRIPTIONS: Record<SerialToolOp, string> = {
  list:
    "List serial ports on this machine and the active serial sessions shared with the user's Serial Port panels (path, state, parameters). Up to 4 ports can be open at once.",
  open:
    "Open a serial port when the session is closed. If the user already has a port open, this succeeds only when path and parameters match exactly; otherwise it fails and you must ask the user. Requires approval.",
  write:
    "Write to the open serial port. encoding=utf-8 sends text (optionally with a line ending); encoding=hex sends raw bytes such as '41 54 0D 0A'. Returns the written byte count and its seq. Requires approval.",
  read:
    "Read the serial log incrementally. Omit sinceSeq to get only the current lastSeq (start reading from now); pass the returned lastSeq next time to continue; sinceSeq=0 reads the whole retained buffer. direction defaults to rx.",
  waitFor:
    "Wait until received data (RX) matches a JavaScript regular expression, e.g. after flashing or resetting a device. By default only data arriving after the call is considered; pass sinceSeq to include earlier data. On timeout or disconnect returns the last 2 KiB of RX.",
  close: "Close the serial port. This also disconnects the user's Serial Port panel. Requires approval.",
  setSignals:
    "Set the DTR/RTS output lines, or run a reset pulse: pulse=esp32 (esptool classic reset into the app) or pulse=arduino (DTR pulse). Use it to reset a device and then serial_wait_for its boot log. RTS cannot be set when RTS/CTS flow control is on. Requires approval.",
};

const READ_ONLY_OPS = new Set<SerialToolOp>(["list", "read", "waitFor"]);

export const SERIAL_TOOLS: SerialToolDefinition[] = Object.entries(TOOL_OPS).map(([name, op]) => ({
  name,
  description: op === "list" || op === "open" ? DESCRIPTIONS[op] : DESCRIPTIONS[op] + PATH_HINT,
  inputSchema: z.toJSONSchema(serialToolArgsSchemas[op], { io: "input" }) as {
    type: "object";
  },
  annotations: READ_ONLY_OPS.has(op)
    ? { title: name, readOnlyHint: true }
    : { title: name, destructiveHint: true },
}));

const requestContextSchema = z
  .object({
    session_id: z.string().trim().min(1).optional(),
    turn_id: z.string().trim().min(1).optional(),
    runtime_scope: z.enum(["main", "subagent"]).default("main"),
  })
  .passthrough();

export type SerialBrokerSend = (
  request: Omit<SerialBrokerRequest, "id" | "token">,
  signal: AbortSignal,
) => Promise<
  { ok: true; result: unknown } | { ok: false; error: { code: string; message: string } }
>;

export interface SerialToolResult {
  content: Array<{ type: "text"; text: string }>;
  isError?: true;
}

function failure(code: string, message: string): SerialToolResult {
  return { content: [{ type: "text", text: `[${code}] ${message}` }], isError: true };
}

/** read 的正文直接给模型看，游标等元数据另起一段，避免把串口文本再转义成 JSON 字符串。 */
function formatSuccess(op: SerialToolOp, result: unknown): SerialToolResult {
  if (op === "read" && result && typeof result === "object" && "text" in result) {
    const { text, ...metadata } = result as { text: string };
    return {
      content: [
        { type: "text", text },
        { type: "text", text: JSON.stringify(metadata) },
      ],
    };
  }
  return { content: [{ type: "text", text: JSON.stringify(result) }] };
}

export async function handleSerialToolCall(input: {
  name: string;
  args: unknown;
  meta: Record<string, unknown> | undefined;
  send: SerialBrokerSend;
  signal: AbortSignal;
}): Promise<SerialToolResult> {
  const op = TOOL_OPS[input.name];
  if (!op) return failure("invalidInput", `Unknown tool: ${input.name}`);
  const parsedArgs = serialToolArgsSchemas[op].safeParse(input.args ?? {});
  if (!parsedArgs.success) return failure("invalidInput", parsedArgs.error.message);
  // 只信任宿主写入的 com.escode/request-context 命名空间；顶层 _meta 可被第三方扩展，不能作为会话凭据。
  const context = requestContextSchema.safeParse(input.meta?.["com.escode/request-context"]);
  if (!context.success || !context.data.session_id) {
    return failure("unavailable", "Serial port tools require an ESCode session context");
  }
  const response = await input.send(
    {
      op,
      args: parsedArgs.data,
      sessionId: context.data.session_id,
      ...(context.data.turn_id ? { turnId: context.data.turn_id } : {}),
      runtimeScope: context.data.runtime_scope,
    } as Omit<SerialBrokerRequest, "id" | "token">,
    input.signal,
  );
  return response.ok
    ? formatSuccess(op, response.result)
    : failure(response.error.code, response.error.message);
}
