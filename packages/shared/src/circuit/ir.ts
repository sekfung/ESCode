import { z } from "zod";

/**
 * Circuit IR v1 与识别草稿（docs/circuit/README.md §4、§5，ADR 0005）。
 * 纯 domain 契约：只定义线格式与运行时校验，不做任何 IO。
 * IR 的唯一所有者是 Host 内的 CircuitService；本模块只被服务、插件、面板和远程访问层共享。
 */

export const CIRCUIT_IR_SCHEMA_VERSION = 1;

/** 识别结果的低置信度阈值（§2 低置信度必须可见）：confidence 低于该值的元素计入摘要并在面板标出。 */
export const CIRCUIT_LOW_CONFIDENCE_THRESHOLD = 0.7;

/** IR 标识：由 CircuitService 生成，对调用方不透明。 */
export const circuitIrIdSchema = z.string().trim().min(1).max(128);

/** 元素 id。KiCad 元素使用源文件里的 UUID；识别元素的最终 id 由服务端生成。 */
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

/** 内容哈希：sha256 十六进制。图片和 PDF 以内容哈希区分版本（§3.2）。 */
export const circuitContentHashSchema = z
  .string()
  .regex(/^[a-f0-9]{64}$/u, "Expected a sha256 hex digest");

export const circuitSourceKindSchema = z.enum([
  "kicad_sch",
  "kicad_pcb",
  "netlist",
  "bom",
  "pdf_vector",
  "image",
]);
export type CircuitSourceKind = z.infer<typeof circuitSourceKindSchema>;

/** 源文件坐标区域；识别结果用它记录原图区域（§4 Origin.region、§8 识别框叠加）。 */
export const circuitRegionSchema = z
  .object({
    page: z.number().int().positive().optional(),
    x: z.number().finite(),
    y: z.number().finite(),
    w: z.number().finite().nonnegative(),
    h: z.number().finite().nonnegative(),
  })
  .strict();
export type CircuitRegion = z.infer<typeof circuitRegionSchema>;

export const circuitAuthorSchema = z
  .object({
    kind: z.enum(["user", "agent"]),
    sessionId: z.string().trim().min(1).max(512).optional(),
  })
  .strict();
export type CircuitAuthor = z.infer<typeof circuitAuthorSchema>;

/**
 * 元素来源。confidence 只能由 CircuitService 写入（§4）：
 * parser 恒为 1，recognition 必须小于 1；correction 的具体取值规则由服务端决定，
 * 本 schema 只要求 correction 带 author（§5 circuit_correct 记录 author=agent）。
 */
export const circuitOriginSchema = z
  .object({
    by: z.enum(["parser", "recognition", "correction"]),
    confidence: z.number().min(0).max(1),
    region: circuitRegionSchema.optional(),
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
    if (origin.by === "recognition" && origin.confidence >= 1)
      context.addIssue({
        code: "custom",
        message: "Recognition origin confidence must stay below 1",
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

/**
 * 识别草稿（§5 circuit_submit_recognition）：去掉 confidence 的 IR 元素。
 * 元素 id 只在草稿内部互相引用；提交后由 CircuitService 生成最终 id 并写回 IR（§4「识别结果由服务端生成」）。
 * 元素一律 .strict()，因此带 confidence 的输入会被拒绝——Agent 不能自己给置信度（§2 规则 2）。
 */
export const circuitDraftOriginSchema = z
  .object({ by: z.literal("recognition"), region: circuitRegionSchema.optional() })
  .strict();
export type CircuitDraftOrigin = z.infer<typeof circuitDraftOriginSchema>;

export const circuitDraftPinSchema = z
  .object({
    number: z.string().trim().min(1).max(64),
    name: z.string().trim().min(1).max(256).optional(),
    netId: circuitElementIdSchema.optional(),
  })
  .strict();

export const circuitDraftComponentSchema = z
  .object({
    id: circuitElementIdSchema,
    ref: z.string().trim().min(1).max(64),
    value: z.string().max(256).optional(),
    partNumber: z.string().max(256).optional(),
    footprint: z.string().max(256).optional(),
    pins: z.array(circuitDraftPinSchema),
    origin: circuitDraftOriginSchema,
  })
  .strict();

export const circuitDraftNetSchema = z
  .object({
    id: circuitElementIdSchema,
    name: z.string().trim().min(1).max(256).optional(),
    pinRefs: z.array(circuitPinRefSchema),
    origin: circuitDraftOriginSchema,
  })
  .strict();

export const circuitDraftBlockSchema = z
  .object({
    id: circuitElementIdSchema,
    name: z.string().trim().min(1).max(256),
    componentIds: z.array(circuitElementIdSchema),
    origin: circuitDraftOriginSchema,
  })
  .strict();

export const circuitRecognitionDraftSchema = z
  .object({
    components: z.array(circuitDraftComponentSchema),
    nets: z.array(circuitDraftNetSchema),
    // 识别不要求划分功能模块；blocks 可由 Agent 通过修正补充（§4）。
    blocks: z.array(circuitDraftBlockSchema).optional(),
  })
  .strict();
export type CircuitRecognitionDraft = z.infer<typeof circuitRecognitionDraftSchema>;

/** 工具结果里的 IR 摘要（§5 circuit_parse / circuit_get / circuit_submit_recognition）。 */
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
