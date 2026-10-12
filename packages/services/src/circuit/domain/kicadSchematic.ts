import {
  atomAt,
  childLists,
  childListsOf,
  firstChild,
  listHead,
  numberOf,
  parseSExpr,
  readPoint,
  readProperties,
  type SExprList,
} from "./sexpr.js";
import { readLibGraphics, readPinLength, type KicadLibGraphic } from "./kicadLibGraphics.js";
import { readPropertyPlacements, type KicadSymbolProperty } from "./kicadSymbolProperty.js";

export type { KicadLibGraphic, KicadSymbolProperty };

/**
 * `.kicad_sch`（KiCad 6/7/8 s-expression）文档模型。
 * 提取 IR 与面板渲染需要的确定性字段：符号实例、库符号引脚、库图形、导线、结点、标签、子图纸。
 * 这里只做「库坐标 → 图纸坐标」的纯几何搬运，不画 SVG；`kicadLayout.ts` 负责拼装面板用的派生视图。
 */

export interface KicadPoint {
  x: number;
  y: number;
}

export interface KicadPlacement extends KicadPoint {
  rotation: number;
}

export interface KicadLibPin {
  number: string;
  name: string;
  /** 引脚连接点（库坐标，单位 mm）。 */
  at: KicadPlacement;
  /** 引脚长度（mm）：从连接点沿 `at.rotation` 方向画进符号体；缺省 0。 */
  length: number;
  /** 0 表示该引脚属于所有单元（如电源引脚）。 */
  unit: number;
}

export interface KicadLibSymbol {
  libId: string;
  power: boolean;
  /** 库符号声明的最大单元号；单单元器件为 1。 */
  unitCount: number;
  pins: KicadLibPin[];
  graphics: KicadLibGraphic[];
}

export interface KicadSchematicSymbol {
  uuid: string;
  libId: string;
  at: KicadPlacement;
  mirror?: "x" | "y";
  unit: number;
  properties: Map<string, string>;
  /** 属性节点顺序与渲染信息：`properties` 只有名字→值，面板画文本还要 `(at …)` 与 `hide`。 */
  propertyPlacements: KicadSymbolProperty[];
  /** KiCad 7/8 的 `(instances (project … (path "/…" (reference "R1") …)))`：实例路径 → 位号。 */
  instanceReferences: Map<string, string>;
  /** 实例上声明的引脚号（KiCad 7/8 的 `(pin "1" (uuid …))`）。 */
  declaredPins: string[];
}

export interface KicadWire {
  points: KicadPoint[];
}

export interface KicadLabel {
  kind: "local" | "global" | "hierarchical";
  text: string;
  at: KicadPlacement;
}

export interface KicadSheetPin {
  name: string;
  /** 图纸引脚在父页图纸框上的位置（父页坐标）。 */
  at: KicadPlacement;
}

export interface KicadSheet {
  uuid: string;
  name?: string;
  fileName: string;
  at: KicadPlacement;
  size: { width: number; height: number };
  pins: KicadSheetPin[];
}

export interface KicadSchematicDocument {
  version?: string;
  page: { width: number; height: number };
  libSymbols: Map<string, KicadLibSymbol>;
  symbols: KicadSchematicSymbol[];
  wires: KicadWire[];
  junctions: KicadPoint[];
  labels: KicadLabel[];
  noConnects: KicadPoint[];
  sheets: KicadSheet[];
  /** KiCad 6 把实例信息放在根图纸的 `(symbol_instances …)` 里。 */
  legacySymbolInstances: Map<string, string>;
  hasBuses: boolean;
}

/** KiCad 的纸张尺寸表（mm），`(paper "User" w h)` 时用文件里的数值。 */
const PAPER_SIZES: Record<string, { width: number; height: number }> = {
  A5: { width: 210, height: 148 },
  A4: { width: 297, height: 210 },
  A3: { width: 420, height: 297 },
  A2: { width: 594, height: 420 },
  A1: { width: 841, height: 594 },
  A0: { width: 1189, height: 841 },
  A: { width: 279.4, height: 215.9 },
  B: { width: 431.8, height: 279.4 },
  C: { width: 558.8, height: 431.8 },
  D: { width: 863.6, height: 558.8 },
  E: { width: 1117.6, height: 863.6 },
  USLetter: { width: 279.4, height: 215.9 },
  USLegal: { width: 355.6, height: 215.9 },
  USLedger: { width: 431.8, height: 279.4 },
};

const DEFAULT_PAPER = PAPER_SIZES.A4!;

export function isKicadSchematicText(text: string): boolean {
  return /^\s*\(kicad_sch\b/u.test(text);
}

export function parseKicadSchematic(text: string): KicadSchematicDocument {
  if (!isKicadSchematicText(text))
    throw new Error("unsupported_format: 不是 KiCad 6+ 原理图（缺 (kicad_sch 开头）");
  let root: SExprList;
  try {
    root = parseSExpr(text);
  } catch (error) {
    throw new Error(`unsupported_format: 原理图不是合法的 s-expression（${(error as Error).message}）`);
  }
  if (listHead(root) !== "kicad_sch") throw new Error("unsupported_format: 不是 KiCad 6+ 原理图");
  const libSymbols = new Map<string, KicadLibSymbol>();
  const libSymbolsNode = firstChild(root, "lib_symbols");
  if (libSymbolsNode) {
    for (const symbol of childListsOf(libSymbolsNode, "symbol")) {
      const parsed = parseLibSymbol(symbol);
      if (parsed) libSymbols.set(parsed.libId, parsed);
    }
  }
  const wires: KicadWire[] = [];
  const junctions: KicadPoint[] = [];
  const labels: KicadLabel[] = [];
  const noConnects: KicadPoint[] = [];
  const sheets: KicadSheet[] = [];
  // 总线不展开，只标记存在（规格 §5 的 P1 边界）；bus_entry 也要算，否则会漏报。
  const hasBuses =
    childListsOf(root, "bus").length > 0 || childListsOf(root, "bus_entry").length > 0;
  for (const child of childListsOf(root, "wire")) {
    const points = childListsOf(firstChild(child, "pts") ?? child, "xy")
      .map((xy) => readPoint(xy))
      .filter((point): point is KicadPlacement => point !== undefined)
      .map((point) => ({ x: point.x, y: point.y }));
    if (points.length >= 2) wires.push({ points });
  }
  for (const child of childListsOf(root, "junction")) {
    const at = readPoint(firstChild(child, "at"));
    if (at) junctions.push({ x: at.x, y: at.y });
  }
  for (const kind of ["label", "global_label", "hierarchical_label"] as const) {
    for (const child of childListsOf(root, kind)) {
      const text = atomAt(child, 1);
      const at = readPoint(firstChild(child, "at"));
      if (text !== undefined && at)
        labels.push({
          kind: kind === "label" ? "local" : kind === "global_label" ? "global" : "hierarchical",
          text,
          at: { x: at.x, y: at.y, rotation: at.rotation ?? 0 },
        });
    }
  }
  for (const child of childListsOf(root, "no_connect")) {
    const at = readPoint(firstChild(child, "at"));
    if (at) noConnects.push({ x: at.x, y: at.y });
  }
  for (const child of childListsOf(root, "sheet")) {
    const parsed = parseSheet(child);
    if (parsed) sheets.push(parsed);
  }
  return {
    version: atomAt(firstChild(root, "version") ?? root, 1),
    page: parsePaper(firstChild(root, "paper")),
    libSymbols,
    symbols: childListsOf(root, "symbol")
      .map(parseSchematicSymbol)
      .filter((symbol): symbol is KicadSchematicSymbol => symbol !== undefined),
    wires,
    junctions,
    labels,
    noConnects,
    sheets,
    legacySymbolInstances: parseLegacySymbolInstances(root),
    hasBuses,
  };
}

function parsePaper(node: SExprList | undefined): { width: number; height: number } {
  const name = atomAt(node ?? { kind: "list", items: [] }, 1);
  if (!name) return DEFAULT_PAPER;
  if (name === "User") {
    const width = numberOf(node, 2);
    const height = numberOf(node, 3);
    if (width !== undefined && height !== undefined) return { width, height };
  }
  return PAPER_SIZES[name] ?? DEFAULT_PAPER;
}

function parseLibSymbol(node: SExprList): KicadLibSymbol | undefined {
  const libId = atomAt(node, 1);
  if (libId === undefined) return undefined;
  const pins: KicadLibPin[] = [];
  const graphics: KicadLibGraphic[] = [];
  let unitCount = 1;
  for (const unitNode of childListsOf(node, "symbol")) {
    const unit = parseUnitIndex(atomAt(unitNode, 1), libId);
    if (unit !== undefined) unitCount = Math.max(unitCount, unit);
    for (const pin of childListsOf(unitNode, "pin")) {
      const at = readPoint(firstChild(pin, "at"));
      const number = atomAt(firstChild(pin, "number") ?? pin, 1);
      if (!at || number === undefined) continue;
      pins.push({
        number,
        name: atomAt(firstChild(pin, "name") ?? pin, 1) ?? "~",
        at: { x: at.x, y: at.y, rotation: at.rotation ?? 0 },
        length: readPinLength(pin),
        unit: unit ?? 0,
      });
    }
    // 子符号名里的单元号决定图形归属：`R_0_1` 的图形所有单元共用，`R_1_1` 只属于单元 1。
    for (const child of childLists(unitNode)) graphics.push(...readLibGraphics(child, unit ?? 0));
  }
  return { libId, power: firstChild(node, "power") !== undefined, unitCount, pins, graphics };
}

/** 单元子符号名形如 `R_1_1`（名字可能含下划线，取末尾两段）。 */
function parseUnitIndex(name: string | undefined, libId: string): number | undefined {
  if (!name) return undefined;
  const match = /_(\d+)_(\d+)$/u.exec(name);
  if (!match) return libId.endsWith(name) ? 0 : undefined;
  const unit = Number(match[1]);
  return Number.isFinite(unit) ? unit : undefined;
}

function parseSchematicSymbol(node: SExprList): KicadSchematicSymbol | undefined {
  const libId = atomAt(firstChild(node, "lib_id") ?? node, 1);
  const uuid = atomAt(firstChild(node, "uuid") ?? node, 1);
  const at = readPoint(firstChild(node, "at"));
  if (libId === undefined || uuid === undefined || !at) return undefined;
  const mirrorNode = firstChild(node, "mirror");
  const mirror = mirrorNode ? atomAt(mirrorNode, 1) : undefined;
  const instanceReferences = new Map<string, string>();
  const instances = firstChild(node, "instances");
  if (instances) {
    for (const project of childListsOf(instances, "project")) {
      for (const path of childListsOf(project, "path")) {
        const reference = atomAt(firstChild(path, "reference") ?? path, 1);
        const pathValue = atomAt(path, 1);
        if (reference !== undefined && pathValue !== undefined)
          instanceReferences.set(pathValue, reference);
      }
    }
  }
  const unit = numberOf(firstChild(node, "unit"), 1) ?? 1;
  return {
    uuid,
    libId,
    at: { x: at.x, y: at.y, rotation: at.rotation ?? 0 },
    ...(mirror === "x" || mirror === "y" ? { mirror } : {}),
    unit,
    properties: readProperties(node),
    propertyPlacements: readPropertyPlacements(node),
    instanceReferences,
    declaredPins: childListsOf(node, "pin")
      .map((pin) => atomAt(pin, 1))
      .filter((value): value is string => value !== undefined),
  };
}

function parseSheet(node: SExprList): KicadSheet | undefined {
  const uuid = atomAt(firstChild(node, "uuid") ?? node, 1);
  const at = readPoint(firstChild(node, "at"));
  const size = readPoint(firstChild(node, "size"));
  const properties = readProperties(node);
  const fileName = properties.get("Sheetfile") ?? properties.get("Sheet file");
  if (uuid === undefined || !at || !size || fileName === undefined) return undefined;
  const pins: KicadSheetPin[] = [];
  for (const pin of childListsOf(node, "pin")) {
    const name = atomAt(pin, 1);
    const pinAt = readPoint(firstChild(pin, "at"));
    if (name !== undefined && pinAt)
      pins.push({ name, at: { x: pinAt.x, y: pinAt.y, rotation: pinAt.rotation ?? 0 } });
  }
  return {
    uuid,
    name: properties.get("Sheetname") ?? properties.get("Sheet name"),
    fileName,
    at: { x: at.x, y: at.y, rotation: at.rotation ?? 0 },
    size: { width: size.x, height: size.y },
    pins,
  };
}

/** KiCad 6 的 `(symbol_instances (path "/…" (reference "R1") …))`。 */
function parseLegacySymbolInstances(root: SExprList): Map<string, string> {
  const instances = new Map<string, string>();
  const node = firstChild(root, "symbol_instances");
  if (!node) return instances;
  for (const path of childListsOf(node, "path")) {
    const pathValue = atomAt(path, 1);
    const reference = atomAt(firstChild(path, "reference") ?? path, 1);
    if (pathValue !== undefined && reference !== undefined) instances.set(pathValue, reference);
  }
  return instances;
}

/**
 * 符号实例变换：库坐标（+Y 向上）→ 图纸坐标（+Y 向下）。
 * 顺序是「先按 `(at … rotation)` 旋转（正交朝向表），再按 `(mirror x|y)` 在图纸坐标轴上镜像，最后平移」。
 *
 * 组合次序以实测为准（KiCad 的朝向矩阵是预乘镜像，等价于旋转后再镜像）：
 * usbc-buck-3v3 的 Q2（AO3401A，`rot 270 mirror x`）三个引脚在先旋转后镜像下正好落在导线端点上，
 * 先镜像后旋转则一个都对不上；三块真实板子的落点匹配率也因此从 79.1% 提高到 81.3%（无解释引脚 3 → 0），
 * 另外两块板没有 mirror×rotation 组合，两种次序结果相同（见 test/circuit/kicadLayout.test.ts）。
 */
export function transformSymbolPoint(
  placement: { at: KicadPlacement; mirror?: "x" | "y" },
  point: { x: number; y: number },
): KicadPoint {
  const rotation = ((placement.at.rotation % 360) + 360) % 360;
  let x: number;
  let y: number;
  switch (rotation) {
    case 90:
      x = -point.y;
      y = -point.x;
      break;
    case 180:
      x = -point.x;
      y = point.y;
      break;
    case 270:
      x = point.y;
      y = point.x;
      break;
    default:
      x = point.x;
      y = -point.y;
      break;
  }
  if (placement.mirror === "x") y = -y;
  if (placement.mirror === "y") x = -x;
  return { x: placement.at.x + x, y: placement.at.y + y };
}

/** 器件位号：多单元器件按 KiCad 习惯加单元字母（U1 的第二个单元是 U1B）。 */
export function symbolReference(
  symbol: KicadSchematicSymbol,
  libSymbol: KicadLibSymbol | undefined,
  instancePath?: string,
): string {
  const reference =
    (instancePath ? symbol.instanceReferences.get(instancePath) : undefined) ??
    symbol.properties.get("Reference") ??
    "?";
  const multiUnit = (libSymbol?.unitCount ?? 1) > 1;
  if (!multiUnit || symbol.unit <= 1) return reference;
  return `${reference}${String.fromCharCode("A".charCodeAt(0) + symbol.unit - 1)}`;
}

/** 实例实际生效的引脚（单元号匹配 + 跨单元共用引脚），带上连接点。 */
export function symbolPins(
  symbol: KicadSchematicSymbol,
  libSymbol: KicadLibSymbol | undefined,
): { number: string; name?: string; point: KicadPoint }[] {
  if (!libSymbol) return [];
  return libSymbol.pins
    .filter((pin) => pin.unit === 0 || pin.unit === symbol.unit)
    .map((pin) => {
      const point = transformSymbolPoint(symbol, pin.at);
      return { number: pin.number, name: pin.name === "~" ? undefined : pin.name, point };
    });
}
