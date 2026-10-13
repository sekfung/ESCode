import { z } from "zod";
import { circuitElementIdSchema, circuitIrIdSchema, circuitWorkspacePathSchema } from "./ir.js";
import { circuitCursorSchema } from "./toolEnvelope.js";
import { cursorField, irIdField, nameSchema, pinNumberSchema } from "./fields.js";

/** 分析类工具：circuit_compare_firmware，以及需要批准的 circuit_export。 */

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

/** `cursor` 与 `circuit_check` / `circuit_query` 同口径：服务端按响应上限分页（§5），拿错游标报 invalid_ir。 */
export const circuitCompareFirmwareRequestSchema = z
  .object({
    ...irIdField,
    files: z.array(circuitWorkspacePathSchema).min(1),
    ...cursorField,
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
 * `::circuit{"irId":"…","title":"…"}` 引用卡片载荷（§8）。只在 Desktop 渲染为卡片；
 * 非 Desktop 环境不渲染（§3.5）。
 */
export const circuitReferenceSchema = z
  .object({
    irId: circuitIrIdSchema,
    title: z.string().trim().min(1).max(250).optional(),
  })
  .strict();
export type CircuitReference = z.infer<typeof circuitReferenceSchema>;
