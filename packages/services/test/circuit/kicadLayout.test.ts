import { existsSync, readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { dirName, joinWorkspacePath } from "../../src/circuit/domain/circuitNaming.js";
import { buildKicadDesign, collectSheetInstances } from "../../src/circuit/domain/kicadDesign.js";
import {
  buildKicadPcbLayout,
  buildKicadSchematicLayout,
} from "../../src/circuit/domain/kicadLayout.js";
import { parseKicadPcb } from "../../src/circuit/domain/kicadPcb.js";
import {
  parseKicadSchematic,
  symbolPins,
  type KicadSchematicDocument,
} from "../../src/circuit/domain/kicadSchematic.js";
import { atomAt, firstChild, listHead, parseSExpr } from "../../src/circuit/domain/sexpr.js";

/**
 * 渲染几何（CircuitLayout）测试：坐标必须与网络推导一致，单位 mm、图纸坐标 +Y 向下。
 * 期望值全部按库坐标手算：先镜像、再按实例 rotation 旋转（0°→(x,-y)、90°→(-y,-x)、270°→(y,x)）、最后平移。
 */

const LIB_SYMBOLS = `(lib_symbols
    (symbol "Device:R"
      (pin_numbers hide) (pin_names (offset 0))
      (property "Reference" "R" (at 2.032 0 90) (effects (font (size 1.27 1.27))))
      (property "Value" "R" (at 0 0 90) (effects (font (size 1.27 1.27))))
      (symbol "R_0_1"
        (rectangle (start -1.016 -2.54) (end 1.016 2.54)
          (stroke (width 0.254) (type default)) (fill (type none))))
      (symbol "R_1_1"
        (polyline (pts (xy -1 0) (xy 0 1.5) (xy 1 0))
          (stroke (width 0) (type default)) (fill (type outline)))
        (circle (center 0 0) (radius 0.5) (stroke (width 0) (type default)) (fill (type none)))
        (pin passive line (at 0 3.81 270) (length 1.27) (name "~" (effects (font (size 1.27 1.27)))) (number "1" (effects (font (size 1.27 1.27)))))
        (pin passive line (at 0 -3.81 90) (length 1.27) (name "~" (effects (font (size 1.27 1.27)))) (number "2" (effects (font (size 1.27 1.27)))))))
    (symbol "power:GND" (power) (pin_names (offset 0))
      (property "Reference" "#PWR" (at 0 -6.35 0) (effects (font (size 1.27 1.27)) hide))
      (property "Value" "GND" (at 0 -3.81 0) (effects (font (size 1.27 1.27))))
      (symbol "GND_0_1" (polyline (pts (xy 0 0) (xy 0 -1.27))))
      (symbol "GND_1_1"
        (pin power_in line (at 0 0 270) (length 0) (name "GND" (effects (font (size 1.27 1.27)))) (number "1" (effects (font (size 1.27 1.27))))))))`;

function schematic(body: string): string {
  return `(kicad_sch (version 20231120) (generator eeschema) (uuid root-uuid) (paper "A4")
  ${LIB_SYMBOLS}
${body}
)`;
}

/** R1：原点放置，Reference 可见。 */
const R1 = `(symbol (lib_id "Device:R") (at 100 100 0) (unit 1) (in_bom yes) (on_board yes) (uuid r1-uuid)
  (property "Reference" "R1" (at 102 99 0) (effects (font (size 1.27 1.27))))
  (property "Value" "10k" (at 102 101 0) (effects (font (size 1.27 1.27))))
  (property "Footprint" "Resistor_SMD:R_0402" (at 100 100 0) (effects (font (size 1.27 1.27)) hide))
  (pin "1" (uuid r1-pin-1)) (pin "2" (uuid r1-pin-2))
  (instances (project "demo" (path "/root-uuid" (reference "R1") (unit 1)))))`;

/** R2：旋转 90°，Reference 隐藏。 */
const R2 = `(symbol (lib_id "Device:R") (at 50 50 90) (unit 1) (in_bom yes) (on_board yes) (uuid r2-uuid)
  (property "Reference" "R2" (at 46 50 90) (effects (font (size 1.27 1.27)) hide))
  (property "Value" "1k" (at 54 50 90) (effects (font (size 1.27 1.27))))
  (pin "1" (uuid r2-pin-1)) (pin "2" (uuid r2-pin-2))
  (instances (project "demo" (path "/root-uuid" (reference "R2") (unit 1)))))`;

/** R3：沿 Y 轴镜像（`(mirror y)` 即 x → -x），旋转为 0，两种次序结果相同。 */
const R3 = `(symbol (lib_id "Device:R") (at 20 20 0) (mirror y) (unit 1) (in_bom yes) (on_board yes) (uuid r3-uuid)
  (property "Reference" "R3" (at 18 19 0) (effects (font (size 1.27 1.27))))
  (property "Value" "2k" (at 18 21 0) (effects (font (size 1.27 1.27))))
  (pin "1" (uuid r3-pin-1)) (pin "2" (uuid r3-pin-2))
  (instances (project "demo" (path "/root-uuid" (reference "R3") (unit 1)))))`;

/** R4：mirror y + 旋转 90°，用来锁死「先旋转、再镜像」的组合次序（两种次序结果不同，见本文件末尾的实测数据）。 */
const R4 = `(symbol (lib_id "Device:R") (at 30 30 90) (mirror y) (unit 1) (in_bom yes) (on_board yes) (uuid r4-uuid)
  (property "Reference" "R4" (at 30 34 90) (effects (font (size 1.27 1.27))))
  (property "Value" "3k" (at 30 26 90) (effects (font (size 1.27 1.27))))
  (pin "1" (uuid r4-pin-1)) (pin "2" (uuid r4-pin-2))
  (instances (project "demo" (path "/root-uuid" (reference "R4") (unit 1)))))`;

const GND1 = `(symbol (lib_id "power:GND") (at 100 103.81 0) (unit 1) (in_bom yes) (on_board yes) (uuid gnd1-uuid)
  (property "Reference" "#PWR01" (at 100 110.16 0) (effects (font (size 1.27 1.27)) hide))
  (property "Value" "GND" (at 100 107.62 0) (effects (font (size 1.27 1.27))))
  (pin "1" (uuid gnd1-pin-1))
  (instances (project "demo" (path "/root-uuid" (reference "#PWR01") (unit 1)))))`;

const WIRES = `(wire (pts (xy 100 96.19) (xy 100 103.81)) (stroke (width 0) (type default)) (uuid w1))
  (junction (at 100 103.81) (diameter 0) (color 0 0 0 0) (uuid j1))
  (label "MID" (at 100 100 0) (effects (font (size 1.27 1.27))) (uuid l1))`;

const ROOT_TEXT = schematic(`${R1}
  ${R2}
  ${R3}
  ${R4}
  ${GND1}
  ${WIRES}`);

/** 手写坐标的浮点比较：KiCad 本身只存到 0.0001 mm，这里留到 1e-6。 */
function expectPoint(actual: { x: number; y: number }, expected: { x: number; y: number }): void {
  expect(actual.x).toBeCloseTo(expected.x, 6);
  expect(actual.y).toBeCloseTo(expected.y, 6);
}

function buildRootLayout() {
  return buildKicadSchematicLayout({
    rootPath: "board.kicad_sch",
    rootUuid: "root-uuid",
    documents: new Map([["board.kicad_sch", parseKicadSchematic(ROOT_TEXT)]]),
    irId: "ir-1",
    revision: 3,
  });
}

describe("原理图渲染几何", () => {
  it("图纸、导线/结点/标签与 IR 对齐字段直接搬运", () => {
    const layout = buildRootLayout();
    expect(layout.irId).toBe("ir-1");
    expect(layout.revision).toBe(3);
    expect(layout.sheets).toHaveLength(1);
    const sheet = layout.sheets[0]!;
    expect(sheet.path).toBe("");
    expect(sheet.name).toBeUndefined();
    expect(sheet.page).toEqual({ width: 297, height: 210 });
    expect(sheet.wires).toEqual([{ points: [{ x: 100, y: 96.19 }, { x: 100, y: 103.81 }] }]);
    expect(sheet.junctions).toEqual([{ x: 100, y: 103.81 }]);
    expect(sheet.labels).toEqual([{ text: "MID", x: 100, y: 100, rotation: 0, kind: "local" }]);
    expect(sheet.symbols.map((symbol) => symbol.elementId)).toEqual([
      "r1-uuid",
      "r2-uuid",
      "r3-uuid",
      "r4-uuid",
      undefined,
    ]);
  });

  it("库图形按单元过滤并变换到图纸坐标（含填充标记）", () => {
    const r1 = buildRootLayout().sheets[0]!.symbols[0]!;
    expect(r1.shapes.map((shape) => [shape.kind, shape.filled])).toEqual([
      ["rectangle", false],
      ["polyline", true],
      ["circle", false],
    ]);
    // rectangle 在 R_0_1（所有单元共用），polyline/circle 在 R_1_1（单元 1）
    const rectangle = r1.shapes[0]!;
    expectPoint(rectangle.points[0]!, { x: 100 - 1.016, y: 100 + 2.54 });
    expectPoint(rectangle.points[1]!, { x: 100 + 1.016, y: 100 - 2.54 });
    const polyline = r1.shapes[1]!;
    expectPoint(polyline.points[0]!, { x: 99, y: 100 });
    expectPoint(polyline.points[1]!, { x: 100, y: 100 - 1.5 });
    expectPoint(polyline.points[2]!, { x: 101, y: 100 });
    const circle = r1.shapes[2]!;
    expectPoint(circle.points[0]!, { x: 100, y: 100 });
    expectPoint(circle.points[1]!, { x: 100.5, y: 100 });
  });

  it("旋转 90° 的实例：图形与引脚都按 KiCad 朝向表变换", () => {
    const r2 = buildRootLayout().sheets[0]!.symbols[1]!;
    expect(r2.x).toBe(50);
    expect(r2.y).toBe(50);
    expect(r2.rotation).toBe(90);
    // 引脚 1 在库坐标 (0, 3.81)：90° → (-y, -x) = (-3.81, 0)；引脚体内侧 (0, 2.54) → (47.46, 50)
    expectPoint(r2.pins[0]!.connection, { x: 46.19, y: 50 });
    expectPoint(r2.pins[0]!.body, { x: 47.46, y: 50 });
    expectPoint(r2.pins[1]!.connection, { x: 53.81, y: 50 });
    expectPoint(r2.pins[1]!.body, { x: 52.54, y: 50 });
    const rectangle = r2.shapes[0]!;
    expectPoint(rectangle.points[0]!, { x: 52.54, y: 51.016 });
    expectPoint(rectangle.points[1]!, { x: 47.46, y: 48.984 });
  });

  it("镜像实例：先镜像再旋转，镜像后的矩形对角点随之交换", () => {
    const r3 = buildRootLayout().sheets[0]!.symbols[2]!;
    expect(r3.mirror).toBe("y");
    expectPoint(r3.pins[0]!.connection, { x: 20, y: 16.19 });
    expectPoint(r3.pins[0]!.body, { x: 20, y: 17.46 });
    const rectangle = r3.shapes[0]!;
    expectPoint(rectangle.points[0]!, { x: 21.016, y: 22.54 });
    expectPoint(rectangle.points[1]!, { x: 18.984, y: 17.46 });
  });

  it("mirror y + 旋转 90°：组合次序为先旋转、再镜像（两种次序结果不同）", () => {
    // 实测依据（packages/services/test/fixtures/circuit 的真实开源板子）：
    // usbc-buck-3v3 的 Q2（AO3401A，rot 270 + mirror x）三个引脚只在「先旋转后镜像」下落在导线端点上；
    // 「先镜像后旋转」的落点一个都对不上。三块板的引脚落点匹配率：79.1% → 81.3%，无解释引脚 3 → 0；
    // 另外两块板没有 mirror×rotation 组合，两种次序同为 91.8% / 99.5%。
    const r4 = buildRootLayout().sheets[0]!.symbols[3]!;
    expect([r4.x, r4.y, r4.rotation, r4.mirror]).toEqual([30, 30, 90, "y"]);
    // 引脚 1 库坐标 (0, 3.81)：旋转 90° → (-3.81, 0)，再沿图纸 Y 轴镜像（x → -x）→ (3.81, 0)
    expectPoint(r4.pins[0]!.connection, { x: 33.81, y: 30 });
    expectPoint(r4.pins[0]!.body, { x: 32.54, y: 30 });
    expectPoint(r4.pins[1]!.connection, { x: 26.19, y: 30 });
    expectPoint(r4.pins[1]!.body, { x: 27.46, y: 30 });
    const rectangle = r4.shapes[0]!;
    expectPoint(rectangle.points[0]!, { x: 27.46, y: 31.016 });
    expectPoint(rectangle.points[1]!, { x: 32.54, y: 28.984 });
  });

  it("文本取 property 的 (at …)：隐藏属性带 hidden，Reference/Value 归类正确", () => {
    const r1 = buildRootLayout().sheets[0]!.symbols[0]!;
    expect(r1.texts).toEqual([
      { kind: "reference", text: "R1", x: 102, y: 99, rotation: 0 },
      { kind: "value", text: "10k", x: 102, y: 101, rotation: 0 },
      { kind: "property", text: "Resistor_SMD:R_0402", x: 100, y: 100, rotation: 0, hidden: true },
    ]);
    const r2 = buildRootLayout().sheets[0]!.symbols[1]!;
    const reference = r2.texts.find((text) => text.kind === "reference")!;
    expect(reference.hidden).toBe(true);
    expect(reference.x).toBe(46);
  });

  it("电源符号没有 elementId 但有 powerName，引脚长度 0 时体端与连接点重合", () => {
    const gnd = buildRootLayout().sheets[0]!.symbols[4]!;
    expect(gnd.elementId).toBeUndefined();
    expect(gnd.powerName).toBe("GND");
    expect(gnd.pins).toHaveLength(1);
    expectPoint(gnd.pins[0]!.connection, { x: 100, y: 103.81 });
    expectPoint(gnd.pins[0]!.body, { x: 100, y: 103.81 });
    // 库图形 polylines 只有 unit 0 的 GND 图形：(0,0)-(0,-1.27) → (100, 103.81)-(100, 105.08)
    expect(gnd.shapes).toHaveLength(1);
    expectPoint(gnd.shapes[0]!.points[1]!, { x: 100, y: 105.08 });
  });

  it("引脚连接点与网络推导（symbolPins）用的点完全一致", () => {
    const document = parseKicadSchematic(ROOT_TEXT);
    const sheet = buildRootLayout().sheets[0]!;
    const wireEnds = new Set(
      sheet.wires.flatMap((wire) => wire.points.map((point) => `${point.x},${point.y}`)),
    );
    for (const layoutSymbol of sheet.symbols) {
      if (layoutSymbol.elementId === undefined) continue;
      const source = document.symbols.find((symbol) => symbol.uuid === layoutSymbol.elementId)!;
      const expected = symbolPins(source, document.libSymbols.get(source.libId));
      // 同一条过滤规则 + 同一个 transformSymbolPoint，因此要求逐位相等（不是近似相等）。
      expect(layoutSymbol.pins.map((pin) => pin.number)).toEqual(expected.map((pin) => pin.number));
      expect(layoutSymbol.pins.map((pin) => pin.connection)).toEqual(
        expected.map((pin) => pin.point),
      );
    }
    // R1 的两个引脚正好落在导线的两个端点上（面板画出的引脚点 = 网表用的点）。
    const r1 = sheet.symbols[0]!;
    for (const pin of r1.pins)
      expect(wireEnds.has(`${pin.connection.x},${pin.connection.y}`)).toBe(true);
  });
});

describe("原理图渲染几何：子图纸", () => {
  const child = schematic(`(symbol (lib_id "Device:R") (at 20 20 0) (unit 1) (in_bom yes) (on_board yes) (uuid child-r1)
  (property "Reference" "R1" (at 22 19 0) (effects (font (size 1.27 1.27))))
  (property "Value" "1k" (at 22 21 0) (effects (font (size 1.27 1.27))))
  (pin "1" (uuid child-p1)) (pin "2" (uuid child-p2)))`);
  const parent = schematic(`${R1}
  (sheet (at 80 75) (size 30 20) (uuid sheet-uuid)
    (property "Sheetname" "sub" (at 80 74.2884 0) (effects (font (size 1.27 1.27)) (justify left bottom)))
    (property "Sheetfile" "sub.kicad_sch" (at 80 95.7507 0) (effects (font (size 1.27 1.27)) (justify left top)))
    (pin "SUB" input (at 85 80 180) (effects (font (size 1.27 1.27)) (justify right)) (uuid sheet-pin-1))
    (instances (project "demo" (path "/root-uuid" (page "2")))))`);
  const input = {
    rootPath: "board.kicad_sch",
    rootUuid: "root-uuid",
    documents: new Map([
      ["board.kicad_sch", parseKicadSchematic(parent)],
      ["sub.kicad_sch", parseKicadSchematic(child)],
    ]),
  };

  it("每张图纸实例产出一个 sheet，子图纸 name 取父页 Sheetname", () => {
    const sheets = buildKicadSchematicLayout(input).sheets;
    expect(sheets.map((sheet) => sheet.path)).toEqual(["", "/sheet-uuid"]);
    expect(sheets[0]!.name).toBeUndefined();
    expect(sheets[1]!.name).toBe("sub");
    expect(sheets[1]!.symbols).toHaveLength(1);
  });

  it("子图纸元素 id 的前缀与 path、IR 组件 id 完全一致", () => {
    const layout = buildKicadSchematicLayout(input);
    const childSheet = layout.sheets[1]!;
    const childUuid = parseKicadSchematic(child).symbols[0]!.uuid;
    expect(childSheet.symbols[0]!.elementId).toBe(`${childSheet.path}/${childUuid}`);
    expect(childSheet.symbols[0]!.elementId).toBe("/sheet-uuid/child-r1");
    const design = buildKicadDesign(input);
    expect(design.components.map((component) => component.id).sort()).toEqual(
      layout.sheets
        .flatMap((sheet) => sheet.symbols.map((symbol) => symbol.elementId))
        .filter((elementId): elementId is string => elementId !== undefined)
        .sort(),
    );
  });
});

describe("PCB 渲染几何", () => {
  const PCB_TEXT = `(kicad_pcb (version 20240108) (generator pcbnew)
    (general (thickness 1.6))
    (paper "A4")
    (layers (0 "F.Cu" signal) (31 "B.Cu" signal) (44 "Edge.Cuts" user))
    (net 0 "")
    (net 1 "GND")
    (gr_rect (start 10 10) (end 110 90)
      (stroke (width 0.05) (type default)) (fill none) (layer "Edge.Cuts") (uuid "gr-1"))
    (footprint "Resistor_SMD:R_0402" (layer "F.Cu") (uuid "fp-r1") (at 30 40)
      (property "Reference" "R1" (at 0 0 0) (layer "F.SilkS") (uuid "fp-r1-ref")
        (effects (font (size 1 1) (thickness 0.15))))
      (pad "1" smd roundrect (at -0.51 0) (size 0.54 0.64) (layers "F.Cu") (net 1 "GND") (uuid "fp-r1-p1"))
      (pad "2" smd roundrect (at 0.51 0) (size 0.54 0.64) (layers "F.Cu") (uuid "fp-r1-p2")))
    (footprint "Package_TO_SOT_SMD:SOT-23" (layer "F.Cu") (uuid "fp-q1") (at 60 70 90)
      (property "Reference" "Q1" (at 0 0 0) (layer "F.SilkS") (uuid "fp-q1-ref")
        (effects (font (size 1 1) (thickness 0.15))))
      (pad "1" smd rect (at -1 -0.95) (size 1.2 1.2) (layers "F.Cu") (uuid "fp-q1-p1")))
)`;

  it("page 取板框，footprint 按 ref 映射回 IR 组件 id", () => {
    const layout = buildKicadPcbLayout({
      pcb: parseKicadPcb(PCB_TEXT),
      irId: "ir-pcb",
      revision: 7,
      components: [{ id: "cmp-r1", ref: "R1" }],
    });
    expect(layout.irId).toBe("ir-pcb");
    expect(layout.revision).toBe(7);
    expect(layout.sheets).toHaveLength(1);
    const sheet = layout.sheets[0]!;
    expect(sheet.path).toBe("");
    expect(sheet.name).toBeUndefined();
    expect(sheet.page).toEqual({ width: 100, height: 80 });
    expect(sheet.wires).toEqual([]);
    expect(sheet.junctions).toEqual([]);
    expect(sheet.labels).toEqual([]);
    expect(sheet.symbols.map((symbol) => symbol.elementId)).toEqual(["cmp-r1", undefined]);
    const r1 = sheet.symbols[0]!;
    expect(r1.libId).toBe("Resistor_SMD:R_0402");
    expect([r1.x, r1.y, r1.rotation]).toEqual([30, 40, 0]);
    expect(r1.shapes).toEqual([]);
    // KicadPcbFootprint 不含 pad 偏移：连接点与引脚体都退到 footprint 原点
    expect(r1.pins).toEqual([
      { number: "1", connection: { x: 30, y: 40 }, body: { x: 30, y: 40 } },
      { number: "2", connection: { x: 30, y: 40 }, body: { x: 30, y: 40 } },
    ]);
    expect(r1.texts).toEqual([{ kind: "reference", text: "R1", x: 30, y: 40, rotation: 0 }]);
  });
});

/** 真实开源板子（来源与许可证见该目录 README）：一块板子一个子目录，目录里放它的 `.kicad_sch`。 */
const FIXTURE_ROOT = fileURLToPath(new URL("../fixtures/circuit/", import.meta.url));

function boardNames(): string[] {
  if (!existsSync(FIXTURE_ROOT)) return [];
  return readdirSync(FIXTURE_ROOT, { withFileTypes: true })
    .filter((entry) => entry.isDirectory())
    .map((entry) => entry.name)
    .sort();
}

/** 一块板子的图纸文本：键与 service 的 workspace 相对路径一致（`<board>/<file>.kicad_sch`）。 */
function boardTexts(board: string): Map<string, string> {
  return new Map(
    readdirSync(join(FIXTURE_ROOT, board))
      .filter((name) => name.endsWith(".kicad_sch"))
      .map((name) => [`${board}/${name}`, readFileSync(join(FIXTURE_ROOT, board, name), "utf8")]),
  );
}

const FIXTURE_BOARDS = boardNames();

/** 板子的根图纸：没有被其它图纸作为 sheet 引用的那些文件。 */
function boardRoots(documents: Map<string, KicadSchematicDocument>): string[] {
  const referenced = new Set<string>();
  for (const [path, document] of documents)
    for (const sheet of document.sheets) {
      const childPath = joinWorkspacePath(dirName(path), sheet.fileName);
      if (documents.has(childPath)) referenced.add(childPath);
    }
  return [...documents.keys()].filter((path) => !referenced.has(path));
}

function documentUuid(text: string): string | undefined {
  const root = parseSExpr(text);
  if (listHead(root) !== "kicad_sch") return undefined;
  const node = firstChild(root, "uuid");
  return node ? atomAt(node, 1) : undefined;
}

/** 0.001 mm 网格的点键：吸收浮点噪声，KiCad 自身只存到 0.0001 mm。 */
function pointKey(point: { x: number; y: number }): string {
  return `${Math.round(point.x * 1000)},${Math.round(point.y * 1000)}`;
}

describe.skipIf(FIXTURE_BOARDS.length === 0)("真实板子：布局与网络推导共用同一套坐标变换", () => {
  for (const board of FIXTURE_BOARDS) {
    it(`${board}：每个非电源符号引脚都落在 KiCad 的合法连接位置`, () => {
      const texts = boardTexts(board);
      const documents = new Map(
        [...texts].map(([path, text]) => [path, parseKicadSchematic(text)]),
      );
      let total = 0;
      let onWireOrLabel = 0;
      const unexplained: string[] = [];
      for (const root of boardRoots(documents)) {
        const rootUuid = documentUuid(texts.get(root)!) ?? "";
        const layout = buildKicadSchematicLayout({ rootPath: root, rootUuid, documents });
        expect(layout.sheets.length).toBeGreaterThan(0);
        expect(layout.sheets[0]!.path).toBe("");
        // 每张图纸的 no_connect 点来自它自己的文档；sheet.path 与实例的相对路径同规则。
        const instances = collectSheetInstances({ rootPath: root, rootUuid, documents }, []);
        const noConnectsByPath = new Map(
          instances.map((instance) => [
            instance.relativePath,
            new Set(instance.document.noConnects.map(pointKey)),
          ]),
        );
        for (const sheet of layout.sheets) {
          const wireSites = new Set<string>();
          for (const wire of sheet.wires)
            for (const point of wire.points) wireSites.add(pointKey(point));
          for (const junction of sheet.junctions) wireSites.add(pointKey(junction));
          for (const label of sheet.labels) wireSites.add(pointKey(label));
          // 引脚直连（KiCad 允许两个引脚端点重合）也是合法连接：统计每个点上的引脚数，
          // 只有「≥2 个引脚共用同一个点」才算解释得了，避免引脚拿自己当连接对象。
          const pinsPerPoint = new Map<string, number>();
          for (const symbol of sheet.symbols)
            for (const pin of symbol.pins) {
              const key = pointKey(pin.connection);
              pinsPerPoint.set(key, (pinsPerPoint.get(key) ?? 0) + 1);
            }
          const noConnects = noConnectsByPath.get(sheet.path)!;
          for (const symbol of sheet.symbols) {
            if (symbol.elementId === undefined) continue;
            for (const pin of symbol.pins) {
              total += 1;
              const key = pointKey(pin.connection);
              if (wireSites.has(key)) onWireOrLabel += 1;
              else if ((pinsPerPoint.get(key) ?? 0) < 2 && !noConnects.has(key))
                unexplained.push(
                  `${symbol.elementId} pin ${pin.number} @ ${pin.connection.x},${pin.connection.y}`,
                );
            }
          }
        }
      }
      expect(total).toBeGreaterThan(0);
      // 任何坐标变换错误都会让引脚落到空白处，所以「无解释引脚」必须为空（比阈值断言更严）。
      expect(unexplained).toEqual([]);
      // 窄口径（只算导线端点/标签/结点）的实测匹配率与差额构成，仅供回归对比：
      // mpu6050-imu 91.8%（45/49，4 个 no_connect）；rp2040-minimal 99.5%（193/194，1 个 no_connect）；
      // usbc-buck-3v3 81.3%（109/134，18 个引脚直连 + 7 个 no_connect）——都在 KiCad 语义下合法。
      expect(onWireOrLabel).toBeGreaterThan(0);
    });
  }
});
