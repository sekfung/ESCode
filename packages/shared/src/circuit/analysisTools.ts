import { z } from "zod";
import { circuitElementIdSchema, circuitIrIdSchema, circuitWorkspacePathSchema } from "./ir.js";
import { circuitCursorSchema } from "./toolEnvelope.js";
import { cursorField, irIdField, nameSchema, pinNumberSchema } from "./fields.js";

/** 分析类工具：circuit_check / circuit_compare_firmware，以及需要批准的 circuit_export。 */

// --- circuit_check ---------------------------------------------------------------

/** P1 内置规则（§6）。 */
export const circuitRuleIdSchema = z.enum([
  "unconnected-pin",
  "single-pin-net",
  "undriven-power",
  "i2c-missing-pullup",
  "missing-decoupling",
  "net-name-conflict",
]);
export type CircuitRuleId = z.infer<typeof circuitRuleIdSchema>;

export const circuitRuleResultSchema = z
  .object({
    ruleId: circuitRuleIdSchema,
    severity: z.enum(["info", "warning", "error"]),
    message: z.string().trim().min(1).max(1024),
    componentIds: z.array(circuitElementIdSchema),
    netIds: z.array(circuitElementIdSchema),
    /** 条目涉及低置信度元素时置 true（§6）。 */
    lowConfidence: z.boolean(),
  })
  .strict();
export type CircuitRuleResult = z.infer<typeof circuitRuleResultSchema>;

/** 规则配置文件位置与加载结果（§6：配置不合法时整份忽略并报告）。 */
export const CIRCUIT_RULE_CONFIG_PATH = ".escode/circuit-rules.json";
export const circuitRuleConfigStatusSchema = z
  .object({
    status: z.enum(["default", "applied", "invalid"]),
    path: circuitWorkspacePathSchema.optional(),
    reason: z.string().trim().min(1).max(1024).optional(),
  })
  .strict();
export type CircuitRuleConfigStatus = z.infer<typeof circuitRuleConfigStatusSchema>;

export const circuitCheckRequestSchema = z.object({ ...irIdField, ...cursorField }).strict();

export const circuitCheckResponseSchema = z
  .object({
    ok: z.literal(true),
    config: circuitRuleConfigStatusSchema,
    items: z.array(circuitRuleResultSchema),
    nextCursor: circuitCursorSchema.optional(),
  })
  .strict();

// --- circuit_compare_firmware ----------------------------------------------------

export const circuitFirmwareCompareKindSchema = z.enum([
  "pin-mismatch",
  "firmware-only",
  "schematic-only",
  "peripheral-conflict",
]);
export type CircuitFirmwareCompareKind = z.infer<typeof circuitFirmwareCompareKindSchema>;

/** 差异条目（§7）：引脚、网络、固件声明与来源行号。 */
export const circuitFirmwareDifferenceSchema = z
  .object({
    kind: circuitFirmwareCompareKindSchema,
    pin: pinNumberSchema.optional(),
    netId: circuitElementIdSchema.optional(),
    netName: nameSchema.optional(),
    componentId: circuitElementIdSchema.optional(),
    firmwareSymbol: nameSchema.optional(),
    file: circuitWorkspacePathSchema,
    line: z.number().int().positive().optional(),
  })
  .strict();
export type CircuitFirmwareDifference = z.infer<typeof circuitFirmwareDifferenceSchema>;

export const circuitCompareFirmwareRequestSchema = z
  .object({
    ...irIdField,
    files: z.array(circuitWorkspacePathSchema).min(1),
  })
  .strict();

export const circuitCompareFirmwareResponseSchema = z
  .object({
    ok: z.literal(true),
    items: z.array(circuitFirmwareDifferenceSchema),
    nextCursor: circuitCursorSchema.optional(),
  })
  .strict();

// --- circuit_export --------------------------------------------------------------

/** 导出快照的文件后缀（§8：文件树里以 `.circuit.json` 打开只读展示）。 */
export const CIRCUIT_EXPORT_SUFFIX = ".circuit.json";

/** 导出需要用户批准（§5），批准由插件层拦截；schema 只描述请求。 */
export const circuitExportRequestSchema = z
  .object({ ...irIdField, path: circuitWorkspacePathSchema })
  .strict();

export const circuitExportResponseSchema = z
  .object({ ok: z.literal(true), path: circuitWorkspacePathSchema })
  .strict();

// --- ::circuit 引用载荷 ----------------------------------------------------------

/**
 * `::circuit{"irId":"…","title":"…"}` 引用卡片载荷（§8）。
 * componentCount / netCount 供 Web 与手机的静态降级卡片显示（§3.5）；Desktop 面板不依赖它们。
 */
export const circuitReferenceSchema = z
  .object({
    irId: circuitIrIdSchema,
    title: z.string().trim().min(1).max(250).optional(),
    componentCount: z.number().int().nonnegative().optional(),
    netCount: z.number().int().nonnegative().optional(),
  })
  .strict();
export type CircuitReference = z.infer<typeof circuitReferenceSchema>;
