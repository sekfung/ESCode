import { z } from "zod";
import { circuitToolRequestSchemas } from "./tools.js";

/**
 * Circuit MCP 工具的整条链路线协议（docs/circuit/README.md §5、§3.5）：
 * MCP server（circuit-plugin）→ Agent 运行时的 broker → Host 反向请求 `interaction/circuit*`
 * → CircuitAgentBridge → CircuitService。工具参数与响应复用 tools.ts 的严格 schema，
 * 这里只补 broker 信封、连接材料 env 与反向方法名（与 `@escode/shared/serial` 同构）。
 */

/** Host 仅在注册了 CircuitService（desktop-local / desktop-attached-remote）时注入该标记。 */
export const ESCODE_HOST_CIRCUIT_ENV = "ESCODE_HOST_CIRCUIT";
export const CIRCUIT_BROKER_SOCKET_ENV = "ESCODE_CIRCUIT_BROKER_SOCKET";
export const CIRCUIT_BROKER_TOKEN_ENV = "ESCODE_CIRCUIT_BROKER_TOKEN";

/**
 * 注册到 Agent 的工具子集（G4 + G3 的分析工具）。`circuit_submit_recognition`（G6）尚未注册，
 * 也不在这里列名：它合入时再补 op、反向方法与插件工具，避免出现没有实现方的协议分支。
 */
export const CIRCUIT_AGENT_TOOL_OPS = [
  "parse",
  "get",
  "query",
  "correct",
  "check",
  "compareFirmware",
  "export",
] as const;
export type CircuitAgentToolOp = (typeof CIRCUIT_AGENT_TOOL_OPS)[number];

// --- broker（MCP server → Agent 运行时） -----------------------------------------

const brokerRequestBase = z.object({
  id: z.string().uuid(),
  token: z.string().min(32),
  runtimeScope: z.enum(["main", "subagent"]),
  sessionId: z.string().trim().min(1),
  turnId: z.string().trim().min(1).optional(),
});

export const circuitBrokerRequestSchema = z.discriminatedUnion("op", [
  brokerRequestBase
    .extend({ op: z.literal("parse"), args: circuitToolRequestSchemas.parse })
    .strict(),
  brokerRequestBase.extend({ op: z.literal("get"), args: circuitToolRequestSchemas.get }).strict(),
  brokerRequestBase
    .extend({ op: z.literal("query"), args: circuitToolRequestSchemas.query })
    .strict(),
  brokerRequestBase
    .extend({ op: z.literal("correct"), args: circuitToolRequestSchemas.correct })
    .strict(),
  brokerRequestBase
    .extend({ op: z.literal("check"), args: circuitToolRequestSchemas.check })
    .strict(),
  brokerRequestBase
    .extend({ op: z.literal("compareFirmware"), args: circuitToolRequestSchemas.compareFirmware })
    .strict(),
  brokerRequestBase
    .extend({ op: z.literal("export"), args: circuitToolRequestSchemas.export })
    .strict(),
]);
export type CircuitBrokerRequest = z.infer<typeof circuitBrokerRequestSchema>;

/** 响应里的 result 是工具响应信封（成功或失败），由调用方按 op 用 tools.ts 的总表校验。 */
export const circuitBrokerResponseSchema = z.discriminatedUnion("ok", [
  z.object({ id: z.string(), ok: z.literal(true), result: z.unknown() }).strict(),
  z
    .object({
      id: z.string(),
      ok: z.literal(false),
      error: z.object({ code: z.string(), message: z.string() }).strict(),
    })
    .strict(),
]);
export type CircuitBrokerResponse = z.infer<typeof circuitBrokerResponseSchema>;

// --- 反向协议（Agent 运行时 → Host） ---------------------------------------------

export const escodeCircuitMethods = {
  parse: "interaction/circuitParse",
  get: "interaction/circuitGet",
  query: "interaction/circuitQuery",
  correct: "interaction/circuitCorrect",
  check: "interaction/circuitCheck",
  compareFirmware: "interaction/circuitCompareFirmware",
  export: "interaction/circuitExport",
} as const satisfies Record<CircuitAgentToolOp, string>;

const protocolParamsBase = z.object({
  requestId: z.string().trim().min(1),
  sessionId: z.string().trim().min(1),
  turnId: z.string().trim().min(1).optional(),
  workspaceKey: z.string().trim().min(1),
  workspacePath: z.string().trim().min(1),
  workspaceIdentity: z.string().trim().min(1).optional(),
  remoteSessionId: z.string().trim().min(1).optional(),
});

export const escodeCircuitMethodParamsSchemas = {
  parse: protocolParamsBase.extend({ args: circuitToolRequestSchemas.parse }).strict(),
  get: protocolParamsBase.extend({ args: circuitToolRequestSchemas.get }).strict(),
  query: protocolParamsBase.extend({ args: circuitToolRequestSchemas.query }).strict(),
  correct: protocolParamsBase.extend({ args: circuitToolRequestSchemas.correct }).strict(),
  check: protocolParamsBase.extend({ args: circuitToolRequestSchemas.check }).strict(),
  compareFirmware: protocolParamsBase
    .extend({ args: circuitToolRequestSchemas.compareFirmware })
    .strict(),
  export: protocolParamsBase.extend({ args: circuitToolRequestSchemas.export }).strict(),
} as const;
export type ESCodeCircuitMethodParams<Op extends CircuitAgentToolOp> = z.infer<
  (typeof escodeCircuitMethodParamsSchemas)[Op]
>;
