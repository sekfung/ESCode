import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { assembleCircuitIr } from "../../src/circuit/domain/irBuild.js";
import { elementsFromEasyedaSchematic } from "../../src/circuit/domain/easyedaDesign.js";
import { parseEasyedaSchematic } from "../../src/circuit/domain/easyedaSchematic.js";

/**
 * EasyEDA `.esch2` 的确定性提取（docs/circuit/README.md §4：器件由 COMPONENT + 属性给出，
 * 网络由导线几何连通 + NET 名合并推出）。
 * fixture 是手写的最小原理图：U1 与 R1 各两个引脚，两条命名导线把 3V3 / GND 连成两个网。
 */

const FIXTURE = fileURLToPath(
  new URL("../fixtures/circuit/easyeda/minimal.esch2", import.meta.url),
);
const text = readFileSync(FIXTURE, "utf-8");

const record = (outer: unknown, inner: string): string => `${JSON.stringify(outer)}||${inner}|`;
const symbolHead = (uuid: string): string =>
  record({ type: "DOCHEAD" }, JSON.stringify({ docType: "SYMBOL", uuid, client: "c1" }));
const pageHead = (uuid: string): string =>
  record({ type: "DOCHEAD" }, JSON.stringify({ docType: "SCH_PAGE", uuid, client: "c1" }));

describe("EasyEDA 原理图解析", () => {
  it("读 SCH_PAGE 的器件、符号引脚与导线", () => {
    const schematic = parseEasyedaSchematic(text);
    expect(schematic.pageUuid).toBe("page-1");
    expect(schematic.components.map((component) => component.ref)).toEqual(["U1", "R1"]);
    const u1 = schematic.components.find((component) => component.ref === "U1")!;
    expect(u1).toMatchObject({
      id: "cmp-u1",
      value: "STM32F103",
      footprint: "QFN-48",
      symbolUuid: "sym-u1",
      partId: "",
      position: { x: 1000, y: 1000 },
      rotation: 0,
      isMirror: false,
    });
    const symbol = schematic.symbols.find((part) => part.symbolUuid === "sym-u1")!;
    expect(symbol.pins.map((pin) => [pin.number, pin.name])).toEqual([
      ["1", "VCC"],
      ["2", "GND"],
    ]);
    expect(schematic.wires.map((wire) => wire.netName)).toEqual(["3V3", "GND"]);
  });

  it("指定不存在的原理图页时报 not_found，没有 SCH_PAGE 时报 unsupported_format", () => {
    expect(() => parseEasyedaSchematic(text, "missing")).toThrow(/^not_found: /u);
    const symbolOnly = [symbolHead("sym-u1")].join("\n");
    expect(() => parseEasyedaSchematic(symbolOnly)).toThrow(/^unsupported_format: /u);
    expect(() => parseEasyedaSchematic("随便一段文本")).toThrow(/^unsupported_format: /u);
  });

  it("按导线连通 + NET 名推出网络，并把 netId 回填到引脚", () => {
    const schematic = parseEasyedaSchematic(text);
    const elements = elementsFromEasyedaSchematic(schematic);
    expect(elements.components.map((component) => component.ref)).toEqual(["R1", "U1"]);
    expect(
      elements.nets.map((net) => ({
        name: net.name,
        pins: net.pinRefs.map((ref) => `${ref.componentId}.${ref.pin}`),
      })),
    ).toEqual([
      { name: "3V3", pins: ["easyeda:cmp-r1.1", "easyeda:cmp-u1.1"] },
      { name: "GND", pins: ["easyeda:cmp-r1.2", "easyeda:cmp-u1.2"] },
    ]);
    const u1 = elements.components.find((component) => component.ref === "U1")!;
    expect(u1.pins.map((pin) => [pin.number, pin.name])).toEqual([
      ["1", "VCC"],
      ["2", "GND"],
    ]);
    expect(u1.pins.every((pin) => pin.netId !== undefined)).toBe(true);
    expect(elements.blocks).toEqual([]);
    expect(elements.diagnostics).toEqual([]);
  });

  it("器件 id 只依赖记录 id，重新解析得到同一份 IR 元素", () => {
    const first = elementsFromEasyedaSchematic(parseEasyedaSchematic(text));
    const again = elementsFromEasyedaSchematic(parseEasyedaSchematic(text));
    expect(again).toEqual(first);
  });

  it("旋转按文档口径绕原点逆时针变换引脚连接点", () => {
    // 引脚在符号坐标 (0, 100)；绕原点逆时针 90° → (-100, 0)，再平移到器件位置。
    const rotated = [
      symbolHead("sym-p"),
      record({ type: "PIN", id: "pp1", ticket: 1 }, '{"partId":"","positionX":0,"positionY":100}'),
      record({ type: "ATTR", id: "pp1n", ticket: 1 }, '{"parentId":"pp1","key":"NUMBER","value":"1"}'),
      pageHead("page-r"),
      record(
        { type: "COMPONENT", id: "cmp-p", ticket: 1 },
        '{"partId":"","positionX":0,"positionY":0,"rotation":90,"isMirror":false}',
      ),
      record({ type: "ATTR", id: "cmp-p-ref", ticket: 1 }, '{"parentId":"cmp-p","key":"Designator","value":"P1"}'),
      record({ type: "ATTR", id: "cmp-p-sym", ticket: 1 }, '{"parentId":"cmp-p","key":"Symbol","value":"sym-p"}'),
      record({ type: "WIRE", id: "w-p", ticket: 1 }, '{"dots":[[-100,0,-50,0]]}'),
      record({ type: "ATTR", id: "w-p-net", ticket: 1 }, '{"parentId":"w-p","key":"NET","value":"SIG"}'),
    ].join("\n");
    const elements = elementsFromEasyedaSchematic(parseEasyedaSchematic(rotated));
    expect(elements.nets).toHaveLength(1);
    expect(elements.nets[0]!.name).toBe("SIG");
    expect(elements.nets[0]!.pinRefs).toEqual([{ componentId: "easyeda:cmp-p", pin: "1" }]);
  });

  it("没有符号可解析的器件没有引脚，也不凭空造网络", () => {
    const orphan = [
      pageHead("page-o"),
      record(
        { type: "COMPONENT", id: "cmp-o", ticket: 1 },
        '{"partId":"","positionX":0,"positionY":0,"rotation":0,"isMirror":false}',
      ),
      record({ type: "ATTR", id: "cmp-o-ref", ticket: 1 }, '{"parentId":"cmp-o","key":"Designator","value":"X1"}'),
      record({ type: "WIRE", id: "w-o", ticket: 1 }, '{"dots":[[0,0,50,0]]}'),
    ].join("\n");
    const elements = elementsFromEasyedaSchematic(parseEasyedaSchematic(orphan));
    expect(elements.components).toHaveLength(1);
    expect(elements.components[0]!.pins).toEqual([]);
    expect(elements.nets).toEqual([]);
  });

  it("装配成 IR 时网络与引脚的引用自洽（跑一遍 IR 校验）", () => {
    const elements = elementsFromEasyedaSchematic(parseEasyedaSchematic(text));
    const ir = assembleCircuitIr({
      irId: "ir:easyeda-minimal",
      revision: 1,
      source: { kind: "easyeda_sch", path: "hardware/minimal.esch2", contentHash: "a".repeat(64) },
      stale: false,
      elements,
    });
    expect(ir.components).toHaveLength(2);
    expect(ir.nets).toHaveLength(2);
    // 校验器只应报「网络只连了一个引脚」这类结构问题，不应报引用悬空。
    expect(ir.diagnostics.map((diagnostic) => diagnostic.code)).not.toContain(
      "unknown-component-reference",
    );
    expect(ir.diagnostics.map((diagnostic) => diagnostic.code)).not.toContain("dangling-net");
  });
});
