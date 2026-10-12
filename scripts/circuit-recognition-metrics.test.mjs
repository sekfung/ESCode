import { describe, expect, it } from "vitest";
import {
  aggregateResults,
  comparisonView,
  compareElements,
  compareRecognition,
  componentsMatch,
  formatRatio,
  netsMatch,
  pinSet,
  standardView,
} from "./lib/circuit-recognition-metrics.mjs";

/**
 * §10.4 识别评测的比对口径（合成数据单测）：器件按位号 + 值/型号，网络按名称 + 引脚集合，
 * 准确率/召回率与两边的未匹配清单，以及 micro 平均的合计。
 */

const standard = {
  components: [
    {
      id: "c-u1",
      ref: "U1",
      value: "MPU-6050",
      pins: [{ number: "1", netId: "net:3V3" }],
      origin: { by: "parser", confidence: 1 },
    },
    { id: "c-c1", ref: "C1", value: "100n", pins: [], origin: { by: "parser", confidence: 1 } },
  ],
  nets: [
    {
      name: "3V3",
      pinRefs: [
        { componentId: "c-u1", pin: "1" },
        { componentId: "c-c1", pin: "1" },
      ],
      origin: { by: "parser", confidence: 1 },
    },
  ],
};

describe("比对视图", () => {
  it("IR 的元素 id 换成位号，草稿 id 换成草稿内的位号", () => {
    const ir = {
      components: [{ id: "c-u1", ref: "U1" }],
      nets: [{ name: "3V3", pinRefs: [{ componentId: "c-u1", pin: "1" }] }],
    };
    expect(standardView(ir).nets[0].pinRefs).toEqual([{ ref: "U1", pin: "1" }]);
    const draft = {
      components: [{ id: "draft-u1", ref: "U1" }],
      nets: [{ name: "3V3", pinRefs: [{ componentId: "draft-u1", pin: "1" }] }],
    };
    expect(comparisonView(draft).nets[0].pinRefs).toEqual([{ ref: "U1", pin: "1" }]);
  });
});

describe("器件匹配（位号 + 值/型号）", () => {
  it("位号相同、值只在两侧都给出时比对，大小写与空白不敏感", () => {
    expect(
      componentsMatch({ ref: "U1", value: "MPU-6050" }, { ref: "u1", value: " mpu-6050 " }),
    ).toBe(true);
    expect(
      componentsMatch({ ref: "U1", value: "MPU-6050" }, { ref: "U1", value: "MPU-6000" }),
    ).toBe(false);
    // 识别结果没读到 value / partNumber 时不算不匹配（模型不一定读得到丝印）。
    expect(componentsMatch({ ref: "U1", value: "MPU-6050" }, { ref: "U1" })).toBe(true);
    expect(
      componentsMatch({ ref: "U1", partNumber: "ABC" }, { ref: "U1", value: "MPU-6050" }),
    ).toBe(true);
    expect(
      componentsMatch({ ref: "U1", partNumber: "ABC" }, { ref: "U1", partNumber: "XYZ" }),
    ).toBe(false);
    expect(componentsMatch({ ref: "U1" }, { ref: "U2" })).toBe(false);
  });
});

describe("网络匹配（名称 + 引脚集合）", () => {
  it("名称与引脚集合都要相同；引脚集合与顺序无关", () => {
    const pins = [
      { ref: "U1", pin: "1" },
      { ref: "C1", pin: "1" },
    ];
    expect(
      netsMatch({ name: "3V3", pinRefs: pins }, { name: "3v3", pinRefs: [...pins].reverse() }),
    ).toBe(true);
    expect(netsMatch({ name: "3V3", pinRefs: pins }, { name: "GND", pinRefs: pins })).toBe(false);
    expect(netsMatch({ name: "3V3", pinRefs: pins }, { name: "3V3", pinRefs: [pins[0]] })).toBe(
      false,
    );
    // 未命名网络按引脚集合比对（KiCad 的 Net-(C1-Pad1) 这类名字模型读不到）。
    expect(netsMatch({ pinRefs: pins }, { name: "Net-(U1-Pad1)", pinRefs: pins })).toBe(false);
    expect(netsMatch({ pinRefs: pins }, { pinRefs: [...pins] })).toBe(true);
    expect([...pinSet({ pinRefs: pins })].sort()).toEqual(["C1.1", "U1.1"]);
  });
});

describe("准确率 / 召回率与未匹配清单", () => {
  const recognized = {
    components: [
      { ref: "U1", value: "MPU-6050" }, // 命中
      { ref: "R9", value: "10k" }, // 误识别
    ],
    nets: [{ name: "3V3", pinRefs: [{ ref: "U1", pin: "1" }] }], // 引脚集合不全 → 未命中
  };

  it("命中数与未匹配清单分两侧给出", () => {
    const result = compareRecognition(standardView(standard), recognized);
    expect(result.components).toMatchObject({ matched: 1, standard: 2, recognized: 2 });
    expect(result.components.recall).toBe(0.5);
    expect(result.components.precision).toBe(0.5);
    expect(result.components.missing).toEqual(["C1"]);
    expect(result.components.extra).toEqual(["R9"]);
    expect(result.nets.matched).toBe(0);
    expect(result.nets.missing).toEqual(["3V3 (C1.1, U1.1)"]);
    expect(result.nets.extra).toEqual(["3V3 (U1.1)"]);
  });

  it("同一识别元素不会被两个标准元素重复计入", () => {
    const result = compareElements(
      [{ ref: "R1" }, { ref: "R1" }],
      [{ ref: "R1" }],
      componentsMatch,
    );
    expect(result.matched).toBe(1);
    expect(result.missing).toEqual(["R1"]);
  });

  it("分母为 0 时给 null（报告显示 n/a），合计用 micro 平均", () => {
    expect(compareElements([], [], componentsMatch).recall).toBeNull();
    expect(formatRatio(null)).toBe("n/a");
    expect(formatRatio(1)).toBe("100.0%");

    const aggregate = aggregateResults([
      {
        components: {
          matched: 1,
          standard: 2,
          recognized: 4,
          precision: 0.25,
          recall: 0.5,
          missing: [],
          extra: [],
        },
        nets: {
          matched: 0,
          standard: 0,
          recognized: 1,
          precision: 0,
          recall: null,
          missing: [],
          extra: [],
        },
      },
      {
        components: {
          matched: 3,
          standard: 4,
          recognized: 4,
          precision: 0.75,
          recall: 0.75,
          missing: [],
          extra: [],
        },
        nets: {
          matched: 1,
          standard: 1,
          recognized: 2,
          precision: 0.5,
          recall: 1,
          missing: [],
          extra: [],
        },
      },
    ]);
    // micro 平均：器件 4/8 与 4/6，不是 (25% + 75%) / 2。
    expect(aggregate.components).toMatchObject({ matched: 4, standard: 6, recognized: 8 });
    expect(aggregate.components.precision).toBe(0.5);
    expect(aggregate.components.recall).toBeCloseTo(4 / 6, 10);
    expect(aggregate.nets).toMatchObject({ matched: 1, standard: 1, recognized: 3 });
    expect(aggregate.nets.precision).toBeCloseTo(1 / 3, 10);
    expect(aggregate.nets.recall).toBe(1);
  });
});
