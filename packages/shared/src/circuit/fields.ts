import { z } from "zod";
import { circuitIrIdSchema } from "./ir.js";
import { circuitCursorSchema } from "./toolEnvelope.js";

/** 工具契约里反复出现的字段片段，避免各文件重复字面量。 */

export const revisionSchema = z.number().int().nonnegative();
export const cursorField = { cursor: circuitCursorSchema.optional() };
export const irIdField = { irId: circuitIrIdSchema };
export const pinNumberSchema = z.string().trim().min(1).max(64);
export const refSchema = z.string().trim().min(1).max(64);
export const nameSchema = z.string().trim().min(1).max(256);
