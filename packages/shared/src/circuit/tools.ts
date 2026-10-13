import { z } from "zod";
import {
  circuitCompareFirmwareRequestSchema,
  circuitCompareFirmwareResponseSchema,
  circuitExportRequestSchema,
  circuitExportResponseSchema,
} from "./analysisTools.js";
import {
  circuitCorrectRequestSchema,
  circuitCorrectResponseSchema,
  circuitGetRequestSchema,
  circuitGetResponseSchema,
  circuitParseRequestSchema,
  circuitParseResponseSchema,
  circuitQueryRequestSchema,
  circuitQueryResponseSchema,
} from "./irTools.js";
import { circuitToolFailureSchema } from "./toolEnvelope.js";
import type { CircuitToolOp } from "./toolEnvelope.js";

/** 六个工具的请求/响应总表；插件、broker 与 Host 用它做统一校验。 */

export const circuitToolRequestSchemas = {
  parse: circuitParseRequestSchema,
  get: circuitGetRequestSchema,
  query: circuitQueryRequestSchema,
  correct: circuitCorrectRequestSchema,
  compareFirmware: circuitCompareFirmwareRequestSchema,
  export: circuitExportRequestSchema,
} as const;

export const circuitToolResponseSchemas = {
  parse: z.discriminatedUnion("ok", [circuitParseResponseSchema, circuitToolFailureSchema]),
  get: z.discriminatedUnion("ok", [circuitGetResponseSchema, circuitToolFailureSchema]),
  query: circuitQueryResponseSchema,
  correct: z.discriminatedUnion("ok", [circuitCorrectResponseSchema, circuitToolFailureSchema]),
  compareFirmware: z.discriminatedUnion("ok", [
    circuitCompareFirmwareResponseSchema,
    circuitToolFailureSchema,
  ]),
  export: z.discriminatedUnion("ok", [circuitExportResponseSchema, circuitToolFailureSchema]),
} as const;

export type CircuitToolRequest<Op extends CircuitToolOp> = z.infer<
  (typeof circuitToolRequestSchemas)[Op]
>;
export type CircuitToolResponse<Op extends CircuitToolOp> = z.infer<
  (typeof circuitToolResponseSchemas)[Op]
>;
