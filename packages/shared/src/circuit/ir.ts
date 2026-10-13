import { z } from "zod";

/**
 * Circuit IR v1（docs/circuit/README.md §4、§5，ADR 0005）。
 * 纯 domain 契约：只定义线格式与运行时校验，不做任何 IO。
 * IR 的唯一所有者是 Host 内的 CircuitService；本模块只被服务、插件、面板和远程访问层共享。
 */

export const CIRCUIT_IR_SCHEMA_VERSION = 1;

/** 低置信度阈值（§2 低置信度必须可见）：confidence 低于该值的元素计入摘要并在面板标出。 */
export const CIRCUIT_LOW_CONFIDENCE_THRESHOLD = 0.7;

/** IR 标识：由 CircuitService 生成，对调用方不透明。 */
export const circuitIrIdSchema = z.string().trim().min(1).max(128);

/** 元素 id。KiCad 元素使用源文件里的 UUID；网表 / BOM 用 `<kind>:<位号>`。 */
export const circuitElementIdSchema = z.string().trim().min(1).max(128);

/**
 * 源文件与固件路径一律是 workspace 相对路径（§4 source.path）。
 * 绝对路径、盘符、UNC 和 `..` 段在此拒绝，服务端再按 workspace 解析。
 */
export const circuitWorkspacePathSchema = z
  .string()
  .trim()
  .min(1)
  .max(1024)
  .refine((value) => {
    if (value.includes("\0") || /[\r\n]/u.test(value)) return false;
    const segments = value.split(/[\\/]/u);
    return (
      !/^(?:\/|[A-Za-z]:[\\/]|\\\\)/u.test(value) &&
      !segments.includes("..") &&
      !segments.includes("")
    );
  }, "Expected a workspace-relative path");

/** 内容哈希：sha256 十六进制（源文件变化检测）。 */
export const circuitContentHashSchema = z
  .string()
  .regex(/^[a-f0-9]{64}$/u, "Expected a sha256 hex digest");

export const circuitSourceKindSchema = z.enum([
  "kicad_sch",
  "kicad_pcb",
  "netlist",
  "bom",
  "easyeda_sch",
]);
export type CircuitSourceKind = z.infer<typeof circuitSourceKindSchema>;

export const circuitAuthorSchema = z
  .object({
    kind: z.enum(["user", "agent"]),
    sessionId: z.string().trim().min(1).max(512).optional(),
  })
  .strict();
export type CircuitAuthor = z.infer<typeof circuitAuthorSchema>;

/**
 * 元素来源。confidence 只能由 CircuitService 写入（§4）：parser 恒为 1；
 * correction 的取值规则由服务端决定（人工修正 1、Agent 新建元素 0.5），
 * 本 schema 只要求 correction 带 author（§5 circuit_correct 记录 author=agent）。
 */
export const circuitOriginSchema = z
  .object({
    by: z.enum(["parser", "correction"]),
    confidence: z.number().min(0).max(1),
    author: circuitAuthorSchema.optional(),
  })
  .strict()
  .superRefine((origin, context) => {
    if (origin.by === "parser" && origin.confidence !== 1)
      context.addIssue({
        code: "custom",
        message: "Parser origin confidence must be 1",
        path: ["confidence"],
      });
    if (origin.by === "correction" && !origin.author)
      context.addIssue({
        code: "custom",
        message: "Correction origin requires an author",
        path: ["author"],
      });
  });
export type CircuitOrigin = z.infer<typeof circuitOriginSchema>;

export const circuitPinSchema = z
  .object({
    number: z.string().trim().min(1).max(64),
    name: z.string().trim().min(1).max(256).optional(),
    netId: circuitElementIdSchema.optional(),
  })
  .strict();
export type CircuitPin = z.infer<typeof circuitPinSchema>;

export const circuitPinRefSchema = z
  .object({ componentId: circuitElementIdSchema, pin: z.string().trim().min(1).max(64) })
  .strict();
export type CircuitPinRef = z.infer<typeof circuitPinRefSchema>;

export const circuitComponentSchema = z
  .object({
    id: circuitElementIdSchema,
    ref: z.string().trim().min(1).max(64),
    value: z.string().max(256).optional(),
    partNumber: z.string().max(256).optional(),
    footprint: z.string().max(256).optional(),
    pins: z.array(circuitPinSchema),
    origin: circuitOriginSchema,
  })
  .strict();
export type CircuitComponent = z.infer<typeof circuitComponentSchema>;

export const circuitNetSchema = z
  .object({
    id: circuitElementIdSchema,
    name: z.string().trim().min(1).max(256).optional(),
    pinRefs: z.array(circuitPinRefSchema),
    origin: circuitOriginSchema,
  })
  .strict();
export type CircuitNet = z.infer<typeof circuitNetSchema>;

export const circuitBlockSchema = z
  .object({
    id: circuitElementIdSchema,
    name: z.string().trim().min(1).max(256),
    componentIds: z.array(circuitElementIdSchema),
    origin: circuitOriginSchema,
  })
  .strict();
export type CircuitBlock = z.infer<typeof circuitBlockSchema>;

/** 校验问题（§4 diagnostics）：悬空网络、位号重复、引脚数不匹配等。 */
export const circuitDiagnosticCodeSchema = z.enum([
  "duplicate-ref",
  "dangling-net",
  "single-pin-net",
  "pin-count-mismatch",
  "unknown-component-reference",
  "unknown-net-reference",
]);
export type CircuitDiagnosticCode = z.infer<typeof circuitDiagnosticCodeSchema>;

export const circuitDiagnosticSeveritySchema = z.enum(["info", "warning", "error"]);
export type CircuitDiagnosticSeverity = z.infer<typeof circuitDiagnosticSeveritySchema>;

export const circuitDiagnosticSchema = z
  .object({
    code: circuitDiagnosticCodeSchema,
    severity: circuitDiagnosticSeveritySchema,
    message: z.string().trim().min(1).max(1024),
    componentIds: z.array(circuitElementIdSchema).max(256).optional(),
    netIds: z.array(circuitElementIdSchema).max(256).optional(),
  })
  .strict();
export type CircuitDiagnostic = z.infer<typeof circuitDiagnosticSchema>;

export const circuitDiagnosticCountsSchema = z
  .object({
    total: z.number().int().nonnegative(),
    error: z.number().int().nonnegative(),
    warning: z.number().int().nonnegative(),
    info: z.number().int().nonnegative(),
  })
  .strict();
export type CircuitDiagnosticCounts = z.infer<typeof circuitDiagnosticCountsSchema>;

export const circuitIrSourceSchema = z
  .object({
    kind: circuitSourceKindSchema,
    path: circuitWorkspacePathSchema,
    contentHash: circuitContentHashSchema,
  })
  .strict();
export type CircuitIrSource = z.infer<typeof circuitIrSourceSchema>;

/** Circuit IR v1 本体（§4）。引用完整性（pinRefs/netId 指向真实元素）由服务端校验并写入 diagnostics，本 schema 只做结构校验。 */
export const circuitIrSchema = z
  .object({
    schemaVersion: z.literal(CIRCUIT_IR_SCHEMA_VERSION),
    irId: circuitIrIdSchema,
    revision: z.number().int().nonnegative(),
    source: circuitIrSourceSchema,
    stale: z.boolean(),
    components: z.array(circuitComponentSchema),
    nets: z.array(circuitNetSchema),
    blocks: z.array(circuitBlockSchema),
    diagnostics: z.array(circuitDiagnosticSchema),
  })
  .strict();
export type CircuitIR = z.infer<typeof circuitIrSchema>;

/** 工具结果里的 IR 摘要（§5 circuit_parse / circuit_get）。 */
export const circuitIrSummarySchema = z
  .object({
    irId: circuitIrIdSchema,
    revision: z.number().int().nonnegative(),
    source: circuitIrSourceSchema,
    stale: z.boolean(),
    componentCount: z.number().int().nonnegative(),
    netCount: z.number().int().nonnegative(),
    blockCount: z.number().int().nonnegative(),
    lowConfidenceCount: z.number().int().nonnegative(),
    diagnostics: circuitDiagnosticCountsSchema,
  })
  .strict();
export type CircuitIrSummary = z.infer<typeof circuitIrSummarySchema>;

/** 低置信度判定：服务端写出的 confidence 低于阈值。 */
export function isLowConfidence(origin: CircuitOrigin): boolean {
  return origin.confidence < CIRCUIT_LOW_CONFIDENCE_THRESHOLD;
}

export function countCircuitDiagnostics(
  diagnostics: readonly CircuitDiagnostic[],
): CircuitDiagnosticCounts {
  const counts = { total: diagnostics.length, error: 0, warning: 0, info: 0 };
  for (const diagnostic of diagnostics) counts[diagnostic.severity] += 1;
  return counts;
}

export function countLowConfidenceElements(ir: CircuitIR): number {
  return [...ir.components, ...ir.nets, ...ir.blocks].filter((element) =>
    isLowConfidence(element.origin),
  ).length;
}

export function toCircuitIrSummary(ir: CircuitIR): CircuitIrSummary {
  return {
    irId: ir.irId,
    revision: ir.revision,
    source: ir.source,
    stale: ir.stale,
    componentCount: ir.components.length,
    netCount: ir.nets.length,
    blockCount: ir.blocks.length,
    lowConfidenceCount: countLowConfidenceElements(ir),
    diagnostics: countCircuitDiagnostics(ir.diagnostics),
  };
}
