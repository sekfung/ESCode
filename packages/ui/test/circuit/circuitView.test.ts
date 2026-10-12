import { describe, expect, it } from "vitest";
import type { CircuitLayoutSheet } from "@escode/services";
import type { CircuitComponent, CircuitNet } from "@escode/shared/circuit";
import {
  arcPath,
  buildHighlight,
  collectLowConfidenceIds,
  componentIdsOfNet,
  layoutViewBox,
  netIdsOfComponent,
  netLabel,
  pinLabel,
  polylinePoints,
  wireTouchesPoints,
} from "@/circuit/domain/circuitView.js";

/** 面板纯函数（规格 §8）：连接高亮范围、低置信度标记与 SVG 几何派生。 */

const component = (
  id: string,
  ref: string,
  pins: { number: string; netId?: string }[],
  confidence = 1,
): CircuitComponent => ({
  id,
  ref,
  pins,
  origin: { by: "parser", confidence },
});

const net = (
  id: string,
  name: string | undefined,
  pinRefs: { componentId: string; pin: string }[],
  confidence = 1,
): CircuitNet => ({
  id,
  ...(name === undefined ? {} : { name }),
  pinRefs,
  origin: { by: "parser", confidence },
});

const U1 = component("uuid-u1", "U1", [{ number: "1", netId: "net:SDA" }, { number: "2" }]);
const R1 = component("uuid-r1", "R1", [{ number: "1", netId: "net:SDA" }, { number: "2", netId: "net:3V3" }]);
const SDA = net("net:SDA", "SDA", [
  { componentId: "uuid-u1", pin: "1" },
  { componentId: "uuid-r1", pin: "1" },
]);
const THREE_V3 = net("net:3V3", "3V3", [{ componentId: "uuid-r1", pin: "2" }]);

describe("电路面板的连接高亮", () => {
  const components = [U1, R1];
  const nets = [SDA, THREE_V3];

  it("选中器件时高亮它所有引脚所在的网络与整条连接上的引脚", () => {
    const highlight = buildHighlight({ componentId: U1.id, sheetPath: "" }, components, nets);
    expect([...highlight.netIds].sort()).toEqual(["net:SDA"]);
    expect([...highlight.componentIds].sort()).toEqual(["uuid-r1", "uuid-u1"]);
    expect([...highlight.pins].sort()).toEqual([
      "uuid-r1\u00001",
      "uuid-u1\u00001",
    ]);
  });

  it("选中网络时只高亮该网络上的引脚", () => {
    const highlight = buildHighlight({ netId: THREE_V3.id, sheetPath: "" }, components, nets);
    expect([...highlight.netIds]).toEqual(["net:3V3"]);
    expect([...highlight.componentIds]).toEqual([R1.id]);
    expect([...highlight.pins]).toEqual(["uuid-r1\u00002"]);
  });

  it("选择不存在的元素时不产生任何高亮", () => {
    const highlight = buildHighlight(
      { componentId: "missing", netId: "missing-net", sheetPath: "" },
      components,
      nets,
    );
    expect(highlight.netIds.size).toBe(0);
    expect(highlight.componentIds.size).toBe(0);
    expect(highlight.pins.size).toBe(0);
  });

  it("网络的引脚与器件集合来自 IR 的 pinRefs", () => {
    expect(componentIdsOfNet(SDA).sort()).toEqual(["uuid-r1", "uuid-u1"]);
    expect(netIdsOfComponent(R1).sort()).toEqual(["net:3V3", "net:SDA"]);
  });
});

describe("低置信度标记", () => {
  it("低于阈值的器件与网络都进集合，恰好等于阈值的不进", () => {
    const low = component("uuid-low", "U2", [], 0.5);
    const boundary = component("uuid-ok", "U3", [], 0.7);
    const lowNet = net("net:low", "LOW", [], 0.2);
    const ids = collectLowConfidenceIds([low, boundary], [lowNet]);
    expect([...ids].sort()).toEqual(["net:low", "uuid-low"]);
  });
});

describe("SVG 几何派生", () => {
  const sheet: CircuitLayoutSheet = {
    path: "",
    page: { width: 297, height: 210 },
    symbols: [
      {
        elementId: "uuid-r1",
        libId: "Device:R",
        unit: 1,
        x: 100,
        y: 100,
        rotation: 0,
        shapes: [
          {
            kind: "rectangle",
            points: [
              { x: 98.984, y: 97.46 },
              { x: 101.016, y: 102.54 },
            ],
            filled: false,
          },
        ],
        pins: [
          { number: "1", connection: { x: 100, y: 96.19 }, body: { x: 100, y: 97.46 } },
          { number: "2", connection: { x: 100, y: 103.81 }, body: { x: 100, y: 102.54 } },
        ],
        texts: [{ kind: "reference", text: "R1", x: 102, y: 99, rotation: 0 }],
      },
    ],
    wires: [{ points: [{ x: 100, y: 103.81 }, { x: 100, y: 106.19 }] }],
    junctions: [],
    labels: [{ text: "MID", x: 100, y: 105, rotation: 0, kind: "local" }],
  };

  it("viewBox 取图纸与内容外接矩形的并集并留 padding", () => {
    expect(layoutViewBox(sheet, 4)).toEqual({ minX: -4, minY: -4, width: 305, height: 218 });
  });

  it("与高亮引脚相连的导线按连接点判定，不相关的导线不误判", () => {
    const pinPoints = [{ x: 100, y: 103.81 }];
    expect(wireTouchesPoints(sheet.wires[0]!, pinPoints)).toBe(true);
    expect(wireTouchesPoints({ points: [{ x: 0, y: 0 }, { x: 1, y: 1 }] }, pinPoints)).toBe(false);
    expect(wireTouchesPoints(sheet.wires[0]!, [])).toBe(false);
  });

  it("圆弧三点共线时退化成折线，否则给出 A 命令", () => {
    expect(
      arcPath([
        { x: 0, y: 0 },
        { x: 1, y: 0 },
        { x: 2, y: 0 },
      ]),
    ).toBe("M0 0 L1 0 L2 0");
    expect(
      arcPath([
        { x: 0, y: 0 },
        { x: 1, y: 1 },
        { x: 2, y: 0 },
      ]),
    ).toMatch(/^M0 0 A1 1 0 [01] [01] 2 0$/u);
  });

  it("折线点串与标签文案保持 KiCad 的原始值", () => {
    expect(polylinePoints(sheet.wires[0]!.points)).toBe("100,103.81 100,106.19");
    expect(netLabel(SDA)).toBe("SDA");
    expect(netLabel(net("net:Net-(R1-Pad2)", undefined, []))).toBe("net:Net-(R1-Pad2)");
    expect(pinLabel({ number: "1" })).toBe("1");
    expect(pinLabel({ number: "1", name: "VCC" })).toBe("1 (VCC)");
  });
});
