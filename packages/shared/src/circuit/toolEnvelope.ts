import { z } from "zod";

/**
 * Circuit MCP 工具契约的公共信封（docs/circuit/README.md §5）。
 * 所有响应都是 { ok: true, ... } | { ok: false, error: { code, message } }。
 */

/** 官方插件内 circuit MCP server 的名称（工具全名为 mcp__circuit__<tool>）。 */
export const CIRCUIT_MCP_SERVER_NAME = "circuit";

export const CIRCUIT_TOOL_NAMES = {
  parse: "circuit_parse",
  get: "circuit_get",
  query: "circuit_query",
  correct: "circuit_correct",
  compareFirmware: "circuit_compare_firmware",
  export: "circuit_export",
} as const;
export type CircuitToolOp = keyof typeof CIRCUIT_TOOL_NAMES;

/** 单次响应字节上限（§5）：超出部分由调用方带 cursor 分页获取。 */
export const CIRCUIT_RESPONSE_MAX_BYTES = 64 * 1024;

/** 分页游标：不透明字符串，内容由 CircuitService 定义，调用方只负责原样回传。 */
export const circuitCursorSchema = z.string().trim().min(1).max(512);

export const circuitErrorCodeSchema = z.enum([
  "unavailable",
  "not_found",
  "unsupported_format",
  "invalid_ir",
  "stale_revision",
  "too_large",
  "io_error",
]);
export type CircuitErrorCode = z.infer<typeof circuitErrorCodeSchema>;

export const circuitToolFailureSchema = z
  .object({
    ok: z.literal(false),
    error: z
      .object({
        code: circuitErrorCodeSchema,
        message: z.string().trim().min(1).max(2048),
      })
      .strict(),
  })
  .strict();
export type CircuitToolFailure = z.infer<typeof circuitToolFailureSchema>;
