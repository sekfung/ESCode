import { describe, expect, it } from "vitest";
import {
  circuitIrSchema,
  circuitRecognitionDraftSchema,
  circuitToolRequestSchemas,
  circuitToolResponseSchemas,
} from "./contract.js";
import {
  exampleCircuitIr,
  exampleExportFailure,
  exampleGetResponse,
  exampleParseRequest,
  exampleRecognitionDraft,
} from "./contract.example.js";

describe("circuit-protocol contract", () => {
  it("accepts the example IR", () => {
    expect(circuitIrSchema.safeParse(exampleCircuitIr).success).toBe(true);
  });

  it("keeps confidence service-owned: parser=1, recognition<1, correction needs an author", () => {
    const parser = exampleCircuitIr.components[0]!;
    expect(
      circuitIrSchema.safeParse({
        ...exampleCircuitIr,
        components: [{ ...parser, origin: { by: "parser", confidence: 0.5 } }],
      }).success,
    ).toBe(false);
    expect(
      circuitIrSchema.safeParse({
        ...exampleCircuitIr,
        components: [{ ...parser, origin: { by: "recognition", confidence: 1 } }],
      }).success,
    ).toBe(false);
    expect(
      circuitIrSchema.safeParse({
        ...exampleCircuitIr,
        components: [{ ...parser, origin: { by: "correction", confidence: 1 } }],
      }).success,
    ).toBe(false);
  });

  it("accepts the recognition draft only without confidence", () => {
    expect(circuitRecognitionDraftSchema.safeParse(exampleRecognitionDraft).success).toBe(true);
    const withConfidence = {
      ...exampleRecognitionDraft,
      components: [
        {
          ...exampleRecognitionDraft.components[0],
          origin: { by: "recognition", confidence: 0.9 },
        },
      ],
    };
    expect(circuitRecognitionDraftSchema.safeParse(withConfidence).success).toBe(false);
  });

  it("round-trips example tool request and responses", () => {
    expect(circuitToolRequestSchemas.parse.safeParse(exampleParseRequest).success).toBe(true);
    expect(circuitToolResponseSchemas.get.safeParse(exampleGetResponse).success).toBe(true);
    expect(circuitToolResponseSchemas.export.safeParse(exampleExportFailure).success).toBe(true);
  });
});
