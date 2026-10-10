import { z } from "zod";

/**
 * Agent 串口工具契约（docs/specs/serial-agent-tools.md）。
 * MCP server、Agent 运行时 broker（TS / Rust）与 Host 共用；参数在 Host 端做最终严格校验。
 */

/** Host 仅在注册了 SerialService（desktop-local）时向 Agent spawn env 注入该标记。 */
export const ZCODE_HOST_SERIAL_ENV = "ZCODE_HOST_SERIAL";
export const SERIAL_BROKER_SOCKET_ENV = "ZCODE_SERIAL_BROKER_SOCKET";
export const SERIAL_BROKER_TOKEN_ENV = "ZCODE_SERIAL_BROKER_TOKEN";
/** 官方插件内 serial MCP server 的名称（工具全名为 mcp__serial__<tool>）。 */
export const SERIAL_MCP_SERVER_NAME = "serial";

export const SERIAL_READ_DEFAULT_BYTES = 8192;
export const SERIAL_READ_MAX_BYTES = 32768;
export const SERIAL_WAIT_DEFAULT_TIMEOUT_MS = 10_000;
export const SERIAL_WAIT_MAX_TIMEOUT_MS = 120_000;
/** 文本或 HEX 字符串上限；解码后的字节数仍受 Service 的 64 KiB 单次写入上限约束。 */
const SERIAL_WRITE_MAX_INPUT_CHARS = 256 * 1024;

const pathSchema = z.string().trim().min(1).max(512);
/** 多串口时指定目标串口；省略时仅当恰好一个会话时使用它（docs/specs/serial-port-debugger-phase3.md）。 */
const optionalPath = { path: pathSchema.optional() };
const seqSchema = z.number().int().nonnegative();

export const serialToolArgsSchemas = {
  list: z.object({}).strict(),
  open: z
    .object({
      path: pathSchema,
      baudRate: z.number().int().positive(),
      dataBits: z.union([z.literal(5), z.literal(6), z.literal(7), z.literal(8)]).default(8),
      parity: z.enum(["none", "even", "odd", "mark", "space"]).default("none"),
      stopBits: z.union([z.literal(1), z.literal(1.5), z.literal(2)]).default(1),
      rtscts: z.boolean().default(false),
    })
    .strict(),
  write: z
    .object({
      ...optionalPath,
      data: z.string().min(1).max(SERIAL_WRITE_MAX_INPUT_CHARS),
      encoding: z.enum(["utf-8", "hex"]).default("utf-8"),
      lineEnding: z.enum(["none", "cr", "lf", "crlf"]).default("none"),
    })
    .strict(),
  read: z
    .object({
      ...optionalPath,
      sinceSeq: seqSchema.optional(),
      direction: z.enum(["rx", "tx", "both"]).default("rx"),
      encoding: z.enum(["utf-8", "gbk", "hex"]).default("utf-8"),
      maxBytes: z
        .number()
        .int()
        .min(1)
        .max(SERIAL_READ_MAX_BYTES)
        .default(SERIAL_READ_DEFAULT_BYTES),
    })
    .strict(),
  waitFor: z
    .object({
      ...optionalPath,
      pattern: z.string().min(1).max(4096),
      flags: z
        .string()
        .regex(/^[imsu]*$/)
        .optional(),
      sinceSeq: seqSchema.optional(),
      timeoutMs: z
        .number()
        .int()
        .min(1)
        .max(SERIAL_WAIT_MAX_TIMEOUT_MS)
        .default(SERIAL_WAIT_DEFAULT_TIMEOUT_MS),
      encoding: z.enum(["utf-8", "gbk"]).default("utf-8"),
    })
    .strict(),
  close: z.object({ ...optionalPath }).strict(),
} as const;

export type SerialToolOp = keyof typeof serialToolArgsSchemas;
export type SerialToolArgs<Op extends SerialToolOp> = z.infer<(typeof serialToolArgsSchemas)[Op]>;
export const SERIAL_TOOL_OPS = Object.keys(serialToolArgsSchemas) as SerialToolOp[];

// --- broker（MCP server → Agent 运行时） -----------------------------------------

const brokerRequestBase = z.object({
  id: z.string().uuid(),
  token: z.string().min(32),
  runtimeScope: z.enum(["main", "subagent"]),
  sessionId: z.string().trim().min(1),
  turnId: z.string().trim().min(1).optional(),
});

export const serialBrokerRequestSchema = z.discriminatedUnion("op", [
  brokerRequestBase.extend({ op: z.literal("list"), args: serialToolArgsSchemas.list }).strict(),
  brokerRequestBase.extend({ op: z.literal("open"), args: serialToolArgsSchemas.open }).strict(),
  brokerRequestBase.extend({ op: z.literal("write"), args: serialToolArgsSchemas.write }).strict(),
  brokerRequestBase.extend({ op: z.literal("read"), args: serialToolArgsSchemas.read }).strict(),
  brokerRequestBase
    .extend({ op: z.literal("waitFor"), args: serialToolArgsSchemas.waitFor })
    .strict(),
  brokerRequestBase.extend({ op: z.literal("close"), args: serialToolArgsSchemas.close }).strict(),
]);
export type SerialBrokerRequest = z.infer<typeof serialBrokerRequestSchema>;

export const serialBrokerResponseSchema = z.discriminatedUnion("ok", [
  z.object({ id: z.string(), ok: z.literal(true), result: z.unknown() }).strict(),
  z
    .object({
      id: z.string(),
      ok: z.literal(false),
      error: z.object({ code: z.string(), message: z.string() }).strict(),
    })
    .strict(),
]);
export type SerialBrokerResponse = z.infer<typeof serialBrokerResponseSchema>;

// --- 反向协议（Agent 运行时 → Host） ---------------------------------------------

export const zcodeSerialMethods = {
  list: "interaction/serialList",
  open: "interaction/serialOpen",
  write: "interaction/serialWrite",
  read: "interaction/serialRead",
  waitFor: "interaction/serialWaitFor",
  close: "interaction/serialClose",
} as const satisfies Record<SerialToolOp, string>;

const protocolParamsBase = z.object({
  requestId: z.string().trim().min(1),
  sessionId: z.string().trim().min(1),
  turnId: z.string().trim().min(1).optional(),
  workspaceKey: z.string().trim().min(1),
  workspacePath: z.string().trim().min(1),
  workspaceIdentity: z.string().trim().min(1).optional(),
  remoteSessionId: z.string().trim().min(1).optional(),
});

export const zcodeSerialMethodParamsSchemas = {
  list: protocolParamsBase.extend({ args: serialToolArgsSchemas.list }).strict(),
  open: protocolParamsBase.extend({ args: serialToolArgsSchemas.open }).strict(),
  write: protocolParamsBase.extend({ args: serialToolArgsSchemas.write }).strict(),
  read: protocolParamsBase.extend({ args: serialToolArgsSchemas.read }).strict(),
  waitFor: protocolParamsBase.extend({ args: serialToolArgsSchemas.waitFor }).strict(),
  close: protocolParamsBase.extend({ args: serialToolArgsSchemas.close }).strict(),
} as const;
export type ZCodeSerialMethodParams<Op extends SerialToolOp> = z.infer<
  (typeof zcodeSerialMethodParamsSchemas)[Op]
>;

const serialStatusSchema = z
  .object({
    state: z.enum(["closed", "opening", "open", "closing", "disconnected", "error"]),
    path: z.string().optional(),
    config: z
      .object({
        baudRate: z.number(),
        dataBits: z.number(),
        parity: z.string(),
        stopBits: z.number(),
        rtscts: z.boolean(),
        autoReconnect: z.boolean(),
      })
      .strict()
      .optional(),
    error: z.object({ code: z.string(), message: z.string() }).strict().optional(),
  })
  .strict();

export const zcodeSerialMethodResultSchemas = {
  list: z
    .object({
      ports: z.array(
        z
          .object({
            path: z.string(),
            manufacturer: z.string().optional(),
            serialNumber: z.string().optional(),
            vendorId: z.string().optional(),
            productId: z.string().optional(),
          })
          .strict(),
      ),
      sessions: z.array(z.object({ path: z.string(), status: serialStatusSchema }).strict()),
    })
    .strict(),
  open: z.object({ reused: z.boolean(), status: serialStatusSchema }).strict(),
  write: z.object({ bytes: z.number().int(), seq: seqSchema }).strict(),
  read: z
    .object({
      text: z.string(),
      lastSeq: seqSchema,
      truncated: z.boolean(),
      evicted: z.boolean(),
      status: serialStatusSchema,
    })
    .strict(),
  waitFor: z.discriminatedUnion("matched", [
    z
      .object({ matched: z.literal(true), match: z.string(), context: z.string(), seq: seqSchema })
      .strict(),
    z
      .object({
        matched: z.literal(false),
        reason: z.enum(["timeout", "disconnected", "cancelled"]),
        tail: z.string(),
        lastSeq: seqSchema,
      })
      .strict(),
  ]),
  close: z.object({ status: serialStatusSchema }).strict(),
} as const;
export type ZCodeSerialMethodResult<Op extends SerialToolOp> = z.infer<
  (typeof zcodeSerialMethodResultSchemas)[Op]
>;

/** Host 拒绝请求时放进 JSON-RPC error.data.code，broker 原样转成工具失败。 */
export type SerialToolErrorCode =
  | "busy"
  | "denied"
  | "notFound"
  | "invalidConfig"
  | "invalidInput"
  | "nativeUnavailable"
  | "notOpen"
  | "io"
  | "unavailable";

/**
 * 反向请求没有通用取消机制：工具调用被取消或 MCP 对端断开时，broker 发送该方法，
 * Host 按 targetRequestId 终止对应的 waitFor 并释放订阅；目标已结束时视为成功（幂等）。
 */
export const zcodeSerialCancelMethod = "interaction/serialCancel";
export const zcodeSerialCancelParamsSchema = z
  .object({
    sessionId: z.string().trim().min(1),
    targetRequestId: z.string().trim().min(1),
  })
  .strict();
export type ZCodeSerialCancelParams = z.infer<typeof zcodeSerialCancelParamsSchema>;
