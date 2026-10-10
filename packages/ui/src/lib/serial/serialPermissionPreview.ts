import {
  buildSerialSendPayload,
  escapeSerialPreview,
  formatSerialHex,
  serialToolArgsSchemas,
} from "@zcode/shared/serial";

/** 审批卡片最多预览的字节数；字节总数单独显示，避免长数据撑开卡片。 */
const PREVIEW_BYTES = 256;
const TOOL_PREFIX = "mcp__serial__";

export type SerialPermissionPreview =
  | { kind: "write"; bytes: number; text: string; hex: string; truncated: boolean }
  | { kind: "open"; path: string; params: string }
  | { kind: "close" }
  /** 参数无法解析（如非法 HEX）；Host 会拒绝该调用，卡片提示而不是猜测内容。 */
  | { kind: "invalid" };

/**
 * 由串口写类工具的输入生成审批预览。与 Host 使用同一份编解码（@zcode/shared/serial），
 * 保证用户批准看到的字节就是实际写出的字节。只读工具与其它 MCP 工具返回 null。
 */
export function buildSerialPermissionPreview(
  toolName: string,
  input: unknown,
): SerialPermissionPreview | null {
  if (!toolName.startsWith(TOOL_PREFIX)) return null;
  const tool = toolName.slice(TOOL_PREFIX.length);
  if (tool === "serial_close") return { kind: "close" };
  if (tool === "serial_open") {
    const parsed = serialToolArgsSchemas.open.safeParse(input ?? {});
    if (!parsed.success) return { kind: "invalid" };
    const { baudRate, dataBits, parity, stopBits, rtscts } = parsed.data;
    const parityLetter = parity[0]!.toUpperCase();
    return {
      kind: "open",
      path: parsed.data.path,
      params: `${baudRate} ${dataBits}${parityLetter}${stopBits}${rtscts ? " RTS/CTS" : ""}`,
    };
  }
  if (tool !== "serial_write") return null;
  const parsed = serialToolArgsSchemas.write.safeParse(input ?? {});
  if (!parsed.success) return { kind: "invalid" };
  const payload = buildSerialSendPayload({
    input: parsed.data.data,
    mode: parsed.data.encoding === "hex" ? "hex" : "text",
    lineEnding: parsed.data.lineEnding,
  });
  if (!payload.ok) return { kind: "invalid" };
  const head = payload.bytes.subarray(0, PREVIEW_BYTES);
  return {
    kind: "write",
    bytes: payload.bytes.byteLength,
    text: escapeSerialPreview(head),
    hex: formatSerialHex(head),
    truncated: payload.bytes.byteLength > PREVIEW_BYTES,
  };
}
