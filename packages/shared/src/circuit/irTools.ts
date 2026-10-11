import { z } from "zod";
import {
  circuitBlockSchema,
  circuitComponentSchema,
  circuitDiagnosticSchema,
  circuitElementIdSchema,
  circuitIrSummarySchema,
  circuitNetSchema,
  circuitRecognitionDraftSchema,
  circuitWorkspacePathSchema,
} from "./ir.js";
import { circuitCursorSchema, circuitToolFailureSchema } from "./toolEnvelope.js";
import { cursorField, irIdField, nameSchema, pinNumberSchema, refSchema, revisionSchema } from "./fields.js";

/** IR 读写类工具：circuit_parse / circuit_submit_recognition / circuit_get / circuit_query / circuit_correct。 */

// --- circuit_parse ---------------------------------------------------------------

/** 首次解析只给 path；源文件已有 IR 时必须带 baseRevision（§3.4 重新解析）。 */
export const circuitParseRequestSchema = z
  .object({
    path: circuitWorkspacePathSchema,
    baseRevision: revisionSchema.optional(),
  })
  .strict();

export const circuitParseResponseSchema = z
  .object({ ok: z.literal(true), summary: circuitIrSummarySchema })
  .strict();

// --- circuit_submit_recognition --------------------------------------------------

/** 识别草稿不带 confidence；source 的 kind / contentHash 由服务端判定并写入 IR。 */
export const circuitSubmitRecognitionRequestSchema = z
  .object({
    path: circuitWorkspacePathSchema,
    draft: circuitRecognitionDraftSchema,
  })
  .strict();

export const circuitSubmitRecognitionResponseSchema = z
  .object({ ok: z.literal(true), summary: circuitIrSummarySchema })
  .strict();

// --- circuit_get -----------------------------------------------------------------

export const circuitGetRequestSchema = z.object({ ...irIdField }).strict();

/** get 返回摘要，并附带校验问题明细（超出响应上限时截断并置 truncated=true）。 */
export const circuitGetResponseSchema = z
  .object({
    ok: z.literal(true),
    summary: circuitIrSummarySchema,
    diagnostics: z.array(circuitDiagnosticSchema),
    truncated: z.boolean(),
  })
  .strict();

// --- circuit_query ---------------------------------------------------------------

/** 按 ref / net / block 查询，或从 componentId(+pin) 出发追踪连接（一跳）。 */
export const circuitQueryRequestSchema = z
  .discriminatedUnion("by", [
    z.object({ ...irIdField, ...cursorField, by: z.literal("ref"), ref: refSchema }).strict(),
    z
      .object({
        ...irIdField,
        ...cursorField,
        by: z.literal("net"),
        netId: circuitElementIdSchema.optional(),
        netName: nameSchema.optional(),
      })
      .strict(),
    z
      .object({
        ...irIdField,
        ...cursorField,
        by: z.literal("block"),
        blockId: circuitElementIdSchema.optional(),
        blockName: nameSchema.optional(),
      })
      .strict(),
    z
      .object({
        ...irIdField,
        ...cursorField,
        by: z.literal("trace"),
        componentId: circuitElementIdSchema,
        pin: pinNumberSchema.optional(),
      })
      .strict(),
  ])
  .superRefine((request, context) => {
    if (request.by === "net" && (request.netId === undefined) === (request.netName === undefined))
      context.addIssue({
        code: "custom",
        message: "Pass exactly one of netId or netName",
        path: ["netId"],
      });
    if (
      request.by === "block" &&
      (request.blockId === undefined) === (request.blockName === undefined)
    )
      context.addIssue({
        code: "custom",
        message: "Pass exactly one of blockId or blockName",
        path: ["blockId"],
      });
  });

/** trace 结果：起点引脚/器件所在网络，以及网络上的全部引脚与器件。 */
export const circuitQueryTraceResultSchema = z
  .object({
    componentId: circuitElementIdSchema,
    pin: pinNumberSchema.optional(),
    netIds: z.array(circuitElementIdSchema),
    nets: z.array(circuitNetSchema),
    components: z.array(circuitComponentSchema),
  })
  .strict();
export type CircuitQueryTraceResult = z.infer<typeof circuitQueryTraceResultSchema>;

const circuitQuerySuccessSchema = z.discriminatedUnion("by", [
  z
    .object({
      ok: z.literal(true),
      by: z.literal("ref"),
      items: z.array(circuitComponentSchema),
      nextCursor: circuitCursorSchema.optional(),
    })
    .strict(),
  z
    .object({
      ok: z.literal(true),
      by: z.literal("net"),
      items: z.array(circuitNetSchema),
      nextCursor: circuitCursorSchema.optional(),
    })
    .strict(),
  z
    .object({
      ok: z.literal(true),
      by: z.literal("block"),
      items: z.array(circuitBlockSchema),
      nextCursor: circuitCursorSchema.optional(),
    })
    .strict(),
  z
    .object({
      ok: z.literal(true),
      by: z.literal("trace"),
      items: z.array(circuitQueryTraceResultSchema),
      nextCursor: circuitCursorSchema.optional(),
    })
    .strict(),
]);

export const circuitQueryResponseSchema = z.union([
  circuitQuerySuccessSchema,
  circuitToolFailureSchema,
]);

// --- circuit_correct -------------------------------------------------------------

/**
 * 修正操作（§8 改位号/型号/值、增删连接；§4 blocks 可由修正补充）。
 * 新元素（connect 新建的网络、add-block 新增的模块）的 id 由服务端生成；
 * connect 的 netName、add-block 的 name 在服务端按名称查找，找不到就创建。
 * 所有写入都必须带 baseRevision，版本落后会被拒绝（§3.3）。
 */
export const circuitCorrectionOpSchema = z
  .discriminatedUnion("op", [
    z
      .object({
        op: z.literal("update-component"),
        componentId: circuitElementIdSchema,
        ref: refSchema.optional(),
        value: z.string().max(256).optional(),
        partNumber: z.string().max(256).optional(),
        footprint: z.string().max(256).optional(),
      })
      .strict(),
    z
      .object({
        op: z.literal("connect"),
        componentId: circuitElementIdSchema,
        pin: pinNumberSchema,
        netId: circuitElementIdSchema.optional(),
        netName: nameSchema.optional(),
      })
      .strict(),
    z
      .object({
        op: z.literal("disconnect"),
        componentId: circuitElementIdSchema,
        pin: pinNumberSchema,
      })
      .strict(),
    z
      .object({
        op: z.literal("add-block"),
        name: nameSchema,
        componentIds: z.array(circuitElementIdSchema).min(1),
      })
      .strict(),
    z
      .object({
        op: z.literal("update-block"),
        blockId: circuitElementIdSchema,
        name: nameSchema.optional(),
        componentIds: z.array(circuitElementIdSchema).optional(),
      })
      .strict(),
    z.object({ op: z.literal("remove-block"), blockId: circuitElementIdSchema }).strict(),
  ])
  .superRefine((operation, context) => {
    const requireField = (present: boolean, message: string) => {
      if (!present) context.addIssue({ code: "custom", message });
    };
    if (operation.op === "update-component")
      requireField(
        operation.ref !== undefined ||
          operation.value !== undefined ||
          operation.partNumber !== undefined ||
          operation.footprint !== undefined,
        "update-component requires at least one field to change",
      );
    if (operation.op === "connect")
      requireField(
        (operation.netId !== undefined) !== (operation.netName !== undefined),
        "connect takes exactly one of netId or netName",
      );
    if (operation.op === "update-block")
      requireField(
        operation.name !== undefined || operation.componentIds !== undefined,
        "update-block requires at least one field to change",
      );
  });
export type CircuitCorrectionOp = z.infer<typeof circuitCorrectionOpSchema>;

export const circuitCorrectRequestSchema = z
  .object({
    ...irIdField,
    baseRevision: revisionSchema,
    ops: z.array(circuitCorrectionOpSchema).min(1),
  })
  .strict();

export const circuitCorrectResponseSchema = z
  .object({ ok: z.literal(true), summary: circuitIrSummarySchema })
  .strict();
