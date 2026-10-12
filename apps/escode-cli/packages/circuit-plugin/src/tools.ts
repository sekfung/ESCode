import {
  CIRCUIT_AGENT_TOOL_OPS,
  CIRCUIT_EXPORT_SUFFIX,
  CIRCUIT_TOOL_NAMES,
  circuitToolFailureSchema,
  circuitToolRequestSchemas,
  circuitToolResponseSchemas,
  type CircuitAgentToolOp,
} from "@escode/shared/circuit";
import { z } from "zod";

/** MCP 工具元数据：readOnlyHint 让只读工具在 plan 模式可用；写类工具标 destructive，plan 模式不放行。 */
export interface CircuitToolDefinition {
  name: string;
  description: string;
  inputSchema: { type: "object"; [key: string]: unknown };
  annotations: { readOnlyHint?: boolean; destructiveHint?: boolean; title: string };
}

const DESCRIPTIONS: Record<CircuitAgentToolOp, string> = {
  parse:
    "Parse a KiCad schematic/PCB, netlist or BOM file into the Circuit IR held by the ESCode Host, and return its summary (irId, revision, component/net counts, diagnostics). Re-parsing an existing IR requires baseRevision; if the source file changed, the IR is marked stale and must be re-parsed this way.",
  submitRecognition:
    "Submit a recognition draft (components/nets you read off a schematic image or vector PDF) for one image/PDF file. The draft carries no confidence: the Host validates it against the IR rules, assigns the final element ids and derives each element's confidence. Elements whose reference designator or net name is found in the text of a vector PDF are treated as deterministic (confidence 1); everything else is scored from the validation result. Tell the user before sending an image or PDF to a model, and prefer this text/segment extraction for vector PDFs. Re-submitting the same content requires baseRevision. Returns irId, revision and a summary with the diagnostics and low-confidence counts.",
  get: "Read one IR: summary, stale flag, low-confidence element count and the full list of diagnostics (truncated=true when the response hit its size limit). Call it after circuit_parse to see what was extracted.",
  query:
    "Query one IR by ref (component), net (by netId or netName), block (by blockId or blockName), or trace the connections of a component (optionally a single pin). Large results are paged: pass the returned nextCursor back to continue reading.",
  correct:
    "Apply corrections to one IR: rename/update a component (ref, value, partNumber, footprint), connect or disconnect a pin, add/update/remove a functional block. Requires the current baseRevision — a write with a stale revision is rejected so nothing is silently merged. Corrections stay visible in the user's circuit panel as agent-authored.",
  check:
    "Run the built-in deterministic rules on one IR (unconnected pin, single-pin net, undriven power, missing I2C pull-up, missing decoupling, net-name conflict) and return the rule config status (default/applied/invalid from .escode/circuit-rules.json) plus items with ruleId, severity, the involved component/net ids and a lowConfidence flag. Read-only. Report these findings separately from your own review comments; page large results by passing nextCursor back as cursor.",
  compareFirmware:
    "Compare one IR against firmware pin declarations for hardware/firmware bring-up: Zephyr devicetree (.dts/.dtsi/.overlay), STM32CubeMX .ioc, or C headers with GPIO_PIN/port macros. Returns pin-mismatch, firmware-only, schematic-only and peripheral-conflict differences with pin, net, firmware symbol, file and line. Read-only. Page large results by passing nextCursor back as cursor.",
  export:
    `Write a read-only snapshot of one IR into the workspace at the given ${CIRCUIT_EXPORT_SUFFIX} path. The snapshot is a derived artifact: it is never read back and editing it does not change the IR. Requires the user's approval.`,
};

/**
 * 只读工具：get/query/check/compareFirmware 不改任何状态，plan 模式可用。
 * correct/export 写状态或 workspace，标 destructive；parse/submitRecognition 只重建 IR 本身、
 * 不改用户文件，因此两者都不标（与 §5 的「需要批准：否」一致）。
 */
const READ_ONLY_OPS = new Set<CircuitAgentToolOp>(["get", "query", "check", "compareFirmware"]);
const DESTRUCTIVE_OPS = new Set<CircuitAgentToolOp>(["correct", "export"]);

interface JsonSchemaNode {
  const?: unknown;
  enum?: unknown;
  oneOf?: unknown;
  anyOf?: unknown;
  properties?: Record<string, JsonSchemaNode | undefined>;
  required?: unknown;
  [key: string]: unknown;
}

/**
 * 工具 inputSchema（模型侧）。parse/get/correct/export 的请求 schema 本身就是对象；
 * `circuit_query` 是判别联合，zod 会输出 oneOf，而宿主侧的 MCP 描述符归一化只在顶层保留 properties——
 * 直接透传 oneOf 会让模型看不到任何参数字段。这里把联合各分支的属性合并成一份扁平对象
 * （判别字段的 const 合并为 enum），字段全部取自 shared 的同一 schema；
 * 真正的判别与互斥校验仍由 Host 侧 circuitToolRequestSchemas 严格执行，两处不会漂移。
 */
function toolInputSchema(schema: z.ZodType): { type: "object"; [key: string]: unknown } {
  const json = z.toJSONSchema(schema, { io: "input" }) as JsonSchemaNode;
  if (json.type === "object" || json.properties) return json as { type: "object" };
  const branches = (json.oneOf ?? json.anyOf) as JsonSchemaNode[] | undefined;
  if (!branches) throw new Error("Circuit tool schema must be an object or a union of objects");
  const names = [...new Set(branches.flatMap((branch) => Object.keys(branch.properties ?? {})))];
  const properties: Record<string, unknown> = {};
  for (const name of names) {
    const variants = branches
      .map((branch) => branch.properties?.[name])
      .filter((variant): variant is JsonSchemaNode => variant !== undefined);
    const literals = [
      ...new Set(
        variants.flatMap((variant) =>
          variant.const !== undefined
            ? [variant.const]
            : Array.isArray(variant.enum)
              ? variant.enum
              : [],
        ),
      ),
    ];
    properties[name] =
      literals.length > 0 &&
      variants.every((variant) => variant.const !== undefined || variant.enum !== undefined)
        ? { type: "string", enum: literals }
        : variants[0];
  }
  const required = names.filter((name) =>
    branches.every((branch) =>
      Array.isArray(branch.required) ? branch.required.includes(name) : false,
    ),
  );
  return { type: "object", properties, required, additionalProperties: false };
}

export const CIRCUIT_TOOLS: CircuitToolDefinition[] = CIRCUIT_AGENT_TOOL_OPS.map((op) => ({
  name: CIRCUIT_TOOL_NAMES[op],
  description: DESCRIPTIONS[op],
  inputSchema: toolInputSchema(circuitToolRequestSchemas[op]) as {
    type: "object";
    [key: string]: unknown;
  },
  annotations: READ_ONLY_OPS.has(op)
    ? { title: CIRCUIT_TOOL_NAMES[op], readOnlyHint: true }
    : DESTRUCTIVE_OPS.has(op)
      ? { title: CIRCUIT_TOOL_NAMES[op], destructiveHint: true }
      : { title: CIRCUIT_TOOL_NAMES[op] },
}));

const requestContextSchema = z
  .object({
    session_id: z.string().trim().min(1).optional(),
    turn_id: z.string().trim().min(1).optional(),
    runtime_scope: z.enum(["main", "subagent"]).default("main"),
  })
  .passthrough();

export type CircuitBrokerSend = (
  request: {
    op: CircuitAgentToolOp;
    args: unknown;
    sessionId: string;
    turnId?: string;
    runtimeScope: "main" | "subagent";
  },
  signal: AbortSignal,
) => Promise<
  { ok: true; result: unknown } | { ok: false; error: { code: string; message: string } }
>;

export interface CircuitToolResult {
  content: Array<{ type: "text"; text: string }>;
  isError?: true;
}

function failure(code: string, message: string): CircuitToolResult {
  return { content: [{ type: "text", text: `[${code}] ${message}` }], isError: true };
}

/**
 * 渲染工具结果（§5）：成功与失败都用 `{ ok, ... }` 信封原文交给模型，
 * 失败信封里的 code 就是 CircuitService 的错误码；只有信封装不下的失败（本地参数、会话上下文、
 * 传输/契约缺陷）才用 `[code] message` 文本，避免伪造规格外的错误码。
 */
function render(result: unknown): CircuitToolResult {
  return { content: [{ type: "text", text: JSON.stringify(result) }] };
}

export async function handleCircuitToolCall(input: {
  name: string;
  args: unknown;
  meta: Record<string, unknown> | undefined;
  send: CircuitBrokerSend;
  signal: AbortSignal;
}): Promise<CircuitToolResult> {
  const op = CIRCUIT_AGENT_TOOL_OPS.find((candidate) => CIRCUIT_TOOL_NAMES[candidate] === input.name);
  if (!op) return failure("invalidInput", `Unknown tool: ${input.name}`);
  const parsedArgs = circuitToolRequestSchemas[op].safeParse(input.args ?? {});
  if (!parsedArgs.success) return failure("invalidInput", parsedArgs.error.message);
  // 只信任宿主写入的 com.escode/request-context 命名空间；顶层 _meta 可被第三方扩展，不能作为会话凭据。
  const context = requestContextSchema.safeParse(input.meta?.["com.escode/request-context"]);
  if (!context.success || !context.data.session_id) {
    return failure("unavailable", "Circuit tools require an ESCode session context");
  }
  const response = await input.send(
    {
      op,
      args: parsedArgs.data,
      sessionId: context.data.session_id,
      ...(context.data.turn_id ? { turnId: context.data.turn_id } : {}),
      runtimeScope: context.data.runtime_scope,
    },
    input.signal,
  );
  if (!response.ok) {
    // broker 侧失败：规格内的错误码进信封，其余（无 broker、传输错误）保持 `[code] message`。
    if (circuitToolFailureSchema.safeParse({ ok: false, error: response.error }).success)
      return {
        content: [{ type: "text", text: JSON.stringify({ ok: false, error: response.error }) }],
        isError: true,
      };
    return failure(response.error.code, response.error.message);
  }
  const parsedResponse = circuitToolResponseSchemas[op].safeParse(response.result);
  if (!parsedResponse.success)
    return failure("io_error", `Unexpected ${input.name} payload from the Host: ${parsedResponse.error.message}`);
  return parsedResponse.data.ok
    ? render(parsedResponse.data)
    : { ...render(parsedResponse.data), isError: true };
}
