import { describe, expect, it } from "vitest";
import {
  parseKicadSchematic,
  symbolPins,
  transformSymbolPoint,
} from "../../src/circuit/domain/kicadSchematic.js";
import { buildKicadDesign } from "../../src/circuit/domain/kicadDesign.js";

/** 手写的最小 KiCad 8 原理图片段：Device:R + power:GND。 */
function schematic(body: string): string {
  return `(kicad_sch (version 20231120) (generator eeschema) (uuid root-uuid) (paper "A4")
  (lib_symbols
    (symbol "Device:R" (pin_numbers hide) (pin_names (offset 0)) (exclude_from_sim no) (in_bom yes) (on_board yes)
      (property "Reference" "R" (at 2.032 0 90) (effects (font (size 1.27 1.27))))
      (property "Value" "R" (at 0 0 90) (effects (font (size 1.27 1.27))))
      (symbol "R_0_1" (rectangle (start -1.016 -2.54) (end 1.016 2.54)))
      (symbol "R_1_1"
        (pin passive line (at 0 3.81 270) (length 1.27) (name "~" (effects (font (size 1.27 1.27)))) (number "1" (effects (font (size 1.27 1.27)))))
        (pin passive line (at 0 -3.81 90) (length 1.27) (name "~" (effects (font (size 1.27 1.27)))) (number "2" (effects (font (size 1.27 1.27)))))))
    (symbol "power:GND" (power) (pin_names (offset 0)) (exclude_from_sim no) (in_bom yes) (on_board yes)
      (property "Reference" "#PWR" (at 0 -6.35 0) (effects (font (size 1.27 1.27)) hide))
      (property "Value" "GND" (at 0 -3.81 0) (effects (font (size 1.27 1.27))))
      (symbol "GND_0_1" (polyline (pts (xy 0 0) (xy 0 -1.27))))
      (symbol "GND_1_1"
        (pin power_in line (at 0 0 270) (length 0) (name "GND" (effects (font (size 1.27 1.27)))) (number "1" (effects (font (size 1.27 1.27))))))))
${body}
)`;
}

const R1 = `(symbol (lib_id "Device:R") (at 100 100 0) (unit 1) (exclude_from_sim no) (in_bom yes) (on_board yes) (dnp no) (uuid r1-uuid)
  (property "Reference" "R1" (at 102 99 0) (effects (font (size 1.27 1.27))))
  (property "Value" "10k" (at 102 101 0) (effects (font (size 1.27 1.27))))
  (property "Footprint" "Resistor_SMD:R_0402" (at 0 0 0) (effects (font (size 1.27 1.27)) hide))
  (pin "1" (uuid r1-pin1)) (pin "2" (uuid r1-pin2))
  (instances (project "demo" (path "/root-uuid" (reference "R1") (unit 1)))))`;

const R2 = `(symbol (lib_id "Device:R") (at 100 110 0) (unit 1) (exclude_from_sim no) (in_bom yes) (on_board yes) (dnp no) (uuid r2-uuid)
  (property "Reference" "R2" (at 102 109 0) (effects (font (size 1.27 1.27))))
  (property "Value" "10k" (at 102 111 0) (effects (font (size 1.27 1.27))))
  (pin "1" (uuid r2-pin1)) (pin "2" (uuid r2-pin2))
  (instances (project "demo" (path "/root-uuid" (reference "R2") (unit 1)))))`;

const GND1 = `(symbol (lib_id "power:GND") (at 100 113.81 0) (unit 1) (exclude_from_sim no) (in_bom yes) (on_board yes) (dnp no) (uuid gnd1-uuid)
  (property "Reference" "#PWR01" (at 100 120 0) (effects (font (size 1.27 1.27)) hide))
  (property "Value" "GND" (at 100 117 0) (effects (font (size 1.27 1.27))))
  (pin "1" (uuid gnd1-pin1))
  (instances (project "demo" (path "/root-uuid" (reference "#PWR01") (unit 1)))))`;

describe("KiCad 原理图解析", () => {
  it("解析符号、引脚连接点与关键属性", () => {
    const document = parseKicadSchematic(schematic(`${R1}\n  (wire (pts (xy 100 103.81) (xy 100 106.19)) (stroke (width 0) (type default)) (uuid w1))`));
    expect(document.page).toEqual({ width: 297, height: 210 });
    const r1 = document.symbols.find((symbol) => symbol.uuid === "r1-uuid")!;
    expect(r1.properties.get("Reference")).toBe("R1");
    expect(r1.properties.get("Value")).toBe("10k");
    const pins = symbolPins(r1, document.libSymbols.get("Device:R"));
    expect(pins.map((pin) => [pin.number, pin.point])).toEqual([
      ["1", { x: 100, y: 96.19 }],
      ["2", { x: 100, y: 103.81 }],
    ]);
    expect(document.wires).toHaveLength(1);
  });

  it("旋转与镜像后的引脚位置符合 KiCad 朝向表", () => {
    const rotation90 = transformSymbolPoint({ at: { x: 10, y: 10, rotation: 90 } }, { x: 0, y: 3.81 });
    expect(rotation90).toEqual({ x: 10 - 3.81, y: 10 });
    const mirrored = transformSymbolPoint(
      { at: { x: 10, y: 10, rotation: 0 }, mirror: "x" },
      { x: 0, y: 3.81 },
    );
    expect(mirrored).toEqual({ x: 10, y: 13.81 });
  });

  it("拒绝非 s-expression 原理图", () => {
    expect(() => parseKicadSchematic("EESchema Schematic File Version 4")).toThrow(/unsupported_format/u);
  });
});

describe("KiCad 网络推导", () => {
  const build = (body: string) => {
    const root = parseKicadSchematic(schematic(body));
    return buildKicadDesign({
      rootPath: "board.kicad_sch",
      rootUuid: "root-uuid",
      documents: new Map([["board.kicad_sch", root]]),
    });
  };

  it("按导线与标签推导网络，电源符号不进器件列表", () => {
    const design = build(`${R1}
  ${R2}
  ${GND1}
  (wire (pts (xy 100 103.81) (xy 100 106.19)) (stroke (width 0) (type default)) (uuid w1))
  (label "MID" (at 100 105 0) (effects (font (size 1.27 1.27)) (justify left bottom)) (uuid l1))
  (global_label "VCC" (shape input) (at 100 96.19 0) (effects (font (size 1.27 1.27)) (justify left)) (uuid l2))`);

    expect(design.components.map((component) => component.ref).sort()).toEqual(["R1", "R2"]);
    const byName = new Map(design.nets.map((net) => [net.name, net]));
    expect([...byName.keys()].sort()).toEqual(["GND", "MID", "VCC"]);
    expect(byName.get("MID")!.pinRefs).toEqual([
      { componentId: "r1-uuid", pin: "2" },
      { componentId: "r2-uuid", pin: "1" },
    ]);
    expect(byName.get("VCC")!.pinRefs).toEqual([{ componentId: "r1-uuid", pin: "1" }]);
    expect(byName.get("GND")!.pinRefs).toEqual([{ componentId: "r2-uuid", pin: "2" }]);

    const r1 = design.components.find((component) => component.ref === "R1")!;
    expect(r1.pins.find((pin) => pin.number === "1")!.netId).toBe(byName.get("VCC")!.id);
    expect(r1.pins.find((pin) => pin.number === "2")!.netId).toBe(byName.get("MID")!.id);
  });

  it("同名 local 标签在同一图纸内合并，未命名网络按 KiCad 习惯命名", () => {
    const design = build(`${R1}
  ${R2}
  (wire (pts (xy 100 96.19) (xy 90 96.19)) (stroke (width 0) (type default)) (uuid w1))
  (wire (pts (xy 90 96.19) (xy 90 90) (xy 80 90)) (stroke (width 0) (type default)) (uuid w2))
  (label "SENSE" (at 85 90 0) (effects (font (size 1.27 1.27))) (uuid l1))
  (wire (pts (xy 100 103.81) (xy 100 106.19)) (stroke (width 0) (type default)) (uuid w3))
  (wire (pts (xy 110 103.81) (xy 110 106.19)) (stroke (width 0) (type default)) (uuid w4))`);

    const names = design.nets.map((net) => net.name).sort();
    // R2.2 没有接线 → 成为单引脚网络（KiCad 网表同样为未连接引脚建网，由 single-pin-net 诊断标出）
    expect(names).toEqual(["Net-(R1-Pad2)", "Net-(R2-Pad2)", "SENSE"]);
  });

  it("导线中间十字交叉不连通，加上结点后才连通", () => {
    const crossing = `(wire (pts (xy 50 50) (xy 60 50)) (stroke (width 0) (type default)) (uuid c1))
  (wire (pts (xy 55 45) (xy 55 55)) (stroke (width 0) (type default)) (uuid c2))
  (label "A" (at 50 50 0) (effects (font (size 1.27 1.27))) (uuid la))
  (label "B" (at 55 45 0) (effects (font (size 1.27 1.27))) (uuid lb))`;
    const separated = build(crossing);
    expect(separated.nets.map((net) => net.name).sort()).toEqual(["A", "B"]);

    const joined = build(`(junction (at 55 50) (diameter 0) (color 0 0 0 0) (uuid j1))\n  ${crossing}`);
    const joinedNet = joined.nets.find((net) => net.pinRefs.length === 0 && net.name);
    expect(joinedNet?.name === "A" || joinedNet?.name === "B").toBe(true);
    expect(joined.nets.filter((net) => net.name === "A" || net.name === "B")).toHaveLength(1);
  });

  it("展开子图纸：hierarchy label 与父页 sheet pin 相连，位号取实例路径", () => {
    const child = schematic(`(symbol (lib_id "Device:R") (at 20 20 0) (unit 1) (exclude_from_sim no) (in_bom yes) (on_board yes) (dnp no) (uuid child-r1)
  (property "Reference" "R1" (at 22 19 0) (effects (font (size 1.27 1.27))))
  (property "Value" "1k" (at 22 21 0) (effects (font (size 1.27 1.27))))
  (pin "1" (uuid child-p1)) (pin "2" (uuid child-p2)))
  (wire (pts (xy 20 16.19) (xy 20 10)) (stroke (width 0) (type default)) (uuid cw1))
  (hierarchical_label "SUB" (shape input) (at 20 10 90) (effects (font (size 1.27 1.27)) (justify left)) (uuid hl1))`);
    const parent = schematic(`${R1}
  (wire (pts (xy 100 96.19) (xy 90 96.19)) (stroke (width 0) (type default)) (uuid pw1))
  (wire (pts (xy 90 96.19) (xy 90 80) (xy 85 80)) (stroke (width 0) (type default)) (uuid pw2))
  (sheet (at 80 75) (size 30 20) (fields_autoplaced yes) (stroke (width 0.1524) (type solid)) (fill (color 0 0 0 0.0000)) (uuid sheet-uuid)
    (property "Sheetname" "sub" (at 80 74.2884 0) (effects (font (size 1.27 1.27)) (justify left bottom)))
    (property "Sheetfile" "sub.kicad_sch" (at 80 95.7507 0) (effects (font (size 1.27 1.27)) (justify left top)))
    (pin "SUB" input (at 85 80 180) (effects (font (size 1.27 1.27)) (justify right)) (uuid sheet-pin-1))
    (instances (project "demo" (path "/root-uuid" (page "2")))))`);

    const design = buildKicadDesign({
      rootPath: "board.kicad_sch",
      rootUuid: "root-uuid",
      documents: new Map([
        ["board.kicad_sch", parseKicadSchematic(parent)],
        ["sub.kicad_sch", parseKicadSchematic(child)],
      ]),
    });

    expect(design.limitations).toEqual([]);
    const subComponent = design.components.find((component) => component.ref === "R1" && component.id.startsWith("/"));
    expect(subComponent?.id).toBe("/sheet-uuid/child-r1");
    const merged = design.nets.find((net) => net.name === "SUB")!;
    // 父页 R1.1 —— 导线 —— sheet pin，与子图纸 hierarchy label —— 子图纸 R1.1 合并成一个网络
    expect(merged.pinRefs.map((pinRef) => `${pinRef.componentId}.${pinRef.pin}`).sort()).toEqual([
      "/sheet-uuid/child-r1.1",
      "r1-uuid.1",
    ]);
  });
});
