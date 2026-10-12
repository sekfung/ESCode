import { describe, expect, it } from "vitest";
import type { CircuitRecognitionDraft } from "@escode/shared/circuit";
import {
  CIRCUIT_RECOGNITION_BASE_CONFIDENCE,
  CIRCUIT_RECOGNITION_MIN_CONFIDENCE,
  recognitionConfidence,
} from "../../src/circuit/domain/recognitionConfidence.js";
import { footprintPinCount } from "../../src/circuit/domain/recognitionFootprint.js";
import { evidenceHasText } from "../../src/circuit/domain/recognitionEvidence.js";
import { buildRecognitionElements } from "../../src/circuit/domain/recognitionDraft.js";

/**
 * G6 的 domain 纯逻辑（docs/circuit/README.md §2 规则 2/3、§4、§5）：
 * 置信度规则（基准 0.9、扣分、夹取、永不等于 1）、矢量 PDF 证据命中、封装引脚数口径、
 * 草稿校验与元素 id 重映射。全部零 IO、可单测。
 */

function draft(
  components: CircuitRecognitionDraft["components"],
  nets: CircuitRecognitionDraft["nets"] = [],
  blocks: NonNullable<CircuitRecognitionDraft["blocks"]> = [],
) {
  return { components, nets, blocks };
}

describe("识别置信度规则（§2 规则 2、§4）", () => {
  it("没有校验问题就是基准 0.9，且识别元素永远拿不到 1", () => {
    expect(recognitionConfidence([]).confidence).toBe(CIRCUIT_RECOGNITION_BASE_CONFIDENCE);
    for (const codes of [[], ["dangling-net"], ["single-pin-net"]] as const)
      expect(recognitionConfidence(codes).confidence).toBeLessThan(1);
  });

  it("按诊断类别扣分：单引脚网络 −0.1、引脚数 −0.2、位号重复 −0.3、悬空引用 −0.3", () => {
    expect(recognitionConfidence(["single-pin-net"]).confidence).toBe(0.8);
    expect(recognitionConfidence(["pin-count-mismatch"]).confidence).toBe(0.7);
    expect(recognitionConfidence(["duplicate-ref"]).confidence).toBe(0.6);
    expect(recognitionConfidence(["unknown-component-reference"]).confidence).toBe(0.6);
    expect(recognitionConfidence(["unknown-net-reference"]).confidence).toBe(0.6);
    // 零引脚网络不在扣分表里：只写诊断，不动置信度。
    expect(recognitionConfidence(["dangling-net"]).confidence).toBe(0.9);
  });

  it("同一类只扣一次、不同类累加，扣到底夹在 0.05", () => {
    const repeated = recognitionConfidence(["single-pin-net", "single-pin-net"]);
    expect(repeated.confidence).toBe(0.8);
    expect(repeated.categories).toEqual(["single-pin-net"]);

    const all = recognitionConfidence([
      "single-pin-net",
      "pin-count-mismatch",
      "duplicate-ref",
      "unknown-component-reference",
    ]);
    expect(all.deducted).toBe(0.9);
    expect(all.confidence).toBe(CIRCUIT_RECOGNITION_MIN_CONFIDENCE);
    expect(all.categories).toEqual([
      "duplicate-ref",
      "pin-count-mismatch",
      "single-pin-net",
      "unknown-component-reference",
    ]);
  });
});

describe("引脚数与封装一致的口径（§4）", () => {
  it("只认白名单封装族的 <族>-<引脚数> 与 PinHeader/PinSocket 的 m x n", () => {
    expect(footprintPinCount("Package_SO:SOIC-8_3.9x4.9mm_P1.27mm")).toBe(8);
    expect(footprintPinCount("Package_SO:TSSOP-16_4.4x5.0mm_P0.65mm")).toBe(16);
    expect(footprintPinCount("Package_DFN_QFN:QFN-32-1EP_5x5mm_P0.5mm_EP3.2x3.2mm")).toBe(32);
    expect(footprintPinCount("Package_TO_SOT_SMD:SOT-23-5")).toBe(5);
    expect(footprintPinCount("Connector_PinHeader_2.54mm:PinHeader_1x04_P2.54mm_Vertical")).toBe(4);
    expect(footprintPinCount("Connector_PinSocket_2.54mm:PinSocket_2x05_P2.54mm_Vertical")).toBe(
      10,
    );
  });

  it("判不出来就不判定（不猜封装库）：SOT-223、自定义封装、空值", () => {
    expect(footprintPinCount(undefined)).toBeUndefined();
    expect(footprintPinCount("")).toBeUndefined();
    expect(footprintPinCount("Package_TO_SOT_SMD:SOT-223-3_TabPin2")).toBeUndefined();
    expect(footprintPinCount("Package_TO_SOT_SMD:SOT-23")).toBeUndefined();
    expect(footprintPinCount("Custom:MyModule")).toBeUndefined();
    expect(footprintPinCount("Package_SO:SOP-0")).toBeUndefined();
  });
});

describe("矢量 PDF 证据匹配（§2 规则 3）", () => {
  const evidence = {
    pageCount: 2,
    texts: [
      { page: 1, text: "U1", x: 100, y: 100 },
      { page: 2, text: "I2C1_SDA", x: 300, y: 400 },
    ],
    segments: [{ page: 1, x1: 90, y1: 100, x2: 140, y2: 100 }],
  };
  const region = { page: 1, x: 96, y: 96, w: 20, h: 20 };

  it("region 附近的文本命中（不区分大小写、按区域尺寸给容差）", () => {
    expect(evidenceHasText(evidence, region, ["U1"])).toBe(true);
    expect(evidenceHasText(evidence, region, ["u1"])).toBe(true);
    // 容差 = max(2, 0.25 × 20) = 5：文字在框外 5 点以内仍算证据。
    expect(evidenceHasText(evidence, { page: 1, x: 104, y: 104, w: 20, h: 20 }, ["U1"])).toBe(true);
    // 框离文字更远（x = 106 起，容差后左边界 101）时不再命中。
    expect(evidenceHasText(evidence, { page: 1, x: 106, y: 106, w: 20, h: 20 }, ["U1"])).toBe(
      false,
    );
  });

  it("页码、文本内容与候选不符时不命中", () => {
    expect(evidenceHasText(evidence, { ...region, page: 2 }, ["U1"])).toBe(false);
    expect(evidenceHasText(evidence, region, ["U2"])).toBe(false);
    expect(evidenceHasText(evidence, region, [])).toBe(false);
    // 第 2 页的文本只在第 2 页的 region 命中；region 不给 page 时任意页都看。
    expect(evidenceHasText(evidence, { page: 2, x: 296, y: 396, w: 20, h: 20 }, ["I2C1_SDA"])).toBe(
      true,
    );
    expect(evidenceHasText(evidence, { x: 296, y: 396, w: 20, h: 20 }, ["I2C1_SDA"])).toBe(true);
  });

  it("没有证据或没有 region 时一律不命中（image 来源没有确定性证据）", () => {
    expect(evidenceHasText(undefined, region, ["U1"])).toBe(false);
    expect(evidenceHasText(evidence, undefined, ["U1"])).toBe(false);
  });
});

describe("草稿校验、id 重映射与来源（§4、§5）", () => {
  const twoPins = [
    { number: "1", name: "VDD", netId: "draft-3v3" },
    { number: "2", name: "SDA", netId: "draft-sda" },
  ];

  it("草稿 id 只作草稿内引用：pinRefs / componentIds / pins[].netId 全部重写成服务端 id", () => {
    const result = buildRecognitionElements({
      draft: draft(
        [
          {
            id: "draft-u1",
            ref: "U1",
            value: "MPU-6050",
            pins: twoPins,
            origin: { by: "recognition", region: { x: 1, y: 2, w: 3, h: 4 } },
          },
          {
            id: "draft-r1",
            ref: "R1",
            pins: [{ number: "1", netId: "draft-sda" }],
            origin: { by: "recognition" },
          },
        ],
        [
          {
            id: "draft-3v3",
            name: "3V3",
            pinRefs: [{ componentId: "draft-u1", pin: "1" }],
            origin: { by: "recognition" },
          },
          {
            id: "draft-sda",
            name: "I2C1_SDA",
            pinRefs: [
              { componentId: "draft-u1", pin: "2" },
              { componentId: "draft-r1", pin: "1" },
            ],
            origin: { by: "recognition" },
          },
        ],
        [
          {
            id: "draft-block",
            name: "Sensor",
            componentIds: ["draft-u1"],
            origin: { by: "recognition" },
          },
        ],
      ),
    });

    expect(result.components.map((component) => component.id)).toEqual([
      "recognition:U1",
      "recognition:R1",
    ]);
    expect(result.nets.map((net) => net.id)).toEqual(["net:3V3", "net:I2C1_SDA"]);
    expect(result.blocks[0]).toMatchObject({
      id: "block:Sensor",
      componentIds: ["recognition:U1"],
    });
    expect(result.nets[1]?.pinRefs).toEqual([
      { componentId: "recognition:U1", pin: "2" },
      { componentId: "recognition:R1", pin: "1" },
    ]);
    expect(result.components[0]?.pins.map((pin) => pin.netId)).toEqual(["net:3V3", "net:I2C1_SDA"]);
    // 交付的 diagnostics 里没有结构性问题，只有单引脚网络的 warning。
    expect(result.diagnostics.map((diagnostic) => diagnostic.code)).toEqual(["single-pin-net"]);
    expect(result.evidenceHits).toBe(0);
    expect(result.components[0]?.origin).toEqual({
      by: "recognition",
      confidence: 0.9,
      region: { x: 1, y: 2, w: 3, h: 4 },
    });
    expect(result.nets[0]?.origin.confidence).toBe(0.8);
    expect(result.diagnostics).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ code: "single-pin-net", netIds: ["net:3V3"] }),
      ]),
    );
  });

  it("位号重复、引脚数与封装不一致、悬空引用都写诊断并扣分", () => {
    const result = buildRecognitionElements({
      draft: draft(
        [
          {
            id: "a",
            ref: "R1",
            footprint: "Package_SO:SOIC-8_3.9x4.9mm_P1.27mm",
            pins: [{ number: "1" }],
            origin: { by: "recognition" },
          },
          {
            id: "b",
            ref: "R1",
            pins: [{ number: "1", netId: "draft-missing" }],
            origin: { by: "recognition" },
          },
        ],
        [
          {
            id: "draft-bad",
            name: "NOPE",
            pinRefs: [{ componentId: "draft-ghost", pin: "9" }],
            origin: { by: "recognition" },
          },
        ],
      ),
    });

    expect(result.components.map((component) => component.id)).toEqual([
      "recognition:R1",
      "recognition:R1#2",
    ]);
    expect(new Set(result.diagnostics.map((diagnostic) => diagnostic.code))).toEqual(
      new Set([
        "duplicate-ref",
        "pin-count-mismatch",
        "single-pin-net",
        "unknown-component-reference",
        "unknown-net-reference",
      ]),
    );
    // 第一个 R1：位号重复 + 引脚数与 SOIC-8 不一致 → 0.9 − 0.3 − 0.2 = 0.4。
    expect(result.components[0]?.origin.confidence).toBe(0.4);
    // 第二个 R1：位号重复 + 引用了不存在的网络 → 0.9 − 0.3 − 0.3 = 0.3。
    expect(result.components[1]?.origin.confidence).toBe(0.3);
    // 网络引用了不存在的器件 → −0.3，且只剩一个引脚 → 再 −0.1；诊断保留而不是丢弃草稿里的引用。
    expect(result.nets[0]).toMatchObject({
      id: "net:NOPE",
      pinRefs: [{ componentId: "draft-ghost", pin: "9" }],
    });
    expect(result.nets[0]?.origin.confidence).toBe(0.5);
  });

  it("矢量 PDF 文本证据命中的元素按确定性解析记 1，其余按扣分规则", () => {
    const result = buildRecognitionElements({
      draft: draft(
        [
          {
            id: "draft-u1",
            ref: "U1",
            pins: [{ number: "1", netId: "draft-3v3" }],
            origin: { by: "recognition", region: { page: 1, x: 100, y: 200, w: 40, h: 20 } },
          },
          {
            id: "draft-r1",
            ref: "R1",
            pins: [{ number: "1", netId: "draft-3v3" }],
            origin: { by: "recognition", region: { page: 1, x: 400, y: 200, w: 40, h: 20 } },
          },
        ],
        [
          {
            id: "draft-3v3",
            name: "3V3",
            pinRefs: [
              { componentId: "draft-u1", pin: "1" },
              { componentId: "draft-r1", pin: "1" },
            ],
            origin: { by: "recognition", region: { page: 1, x: 110, y: 210, w: 30, h: 10 } },
          },
        ],
      ),
      evidence: {
        pageCount: 1,
        texts: [
          { page: 1, text: "U1", x: 118, y: 208 },
          { page: 1, text: "3V3", x: 120, y: 214 },
        ],
        segments: [{ page: 1, x1: 100, y1: 210, x2: 140, y2: 210 }],
      },
    });

    expect(result.evidenceHits).toBe(2);
    expect(result.components[0]?.origin).toEqual({
      by: "parser",
      confidence: 1,
      region: { page: 1, x: 100, y: 200, w: 40, h: 20 },
    });
    expect(result.nets[0]?.origin).toEqual({
      by: "parser",
      confidence: 1,
      region: { page: 1, x: 110, y: 210, w: 30, h: 10 },
    });
    // R1 不在证据里 → 识别元素，没有校验问题 → 0.9。
    expect(result.components[1]?.origin).toEqual({
      by: "recognition",
      confidence: 0.9,
      region: { page: 1, x: 400, y: 200, w: 40, h: 20 },
    });
  });
});
