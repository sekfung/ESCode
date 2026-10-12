/**
 * `.kicad_pcb` 解析（KiCad 6/7/8 的 s-expression）。
 *
 * 版本差异依据（KiCad 官方文件格式 + 手写片段验证）：
 * - 元素 id：KiCad 6 写 `(tstamp <uuid>)`，KiCad 7/8 写 `(uuid "<uuid>")`；优先 uuid，其次 tstamp。
 * - 元件属性：KiCad 6 用 `(fp_text reference "R1" …)` / `(fp_text value "10k" …)`；
 *   KiCad 7/8 改成 `(property "Reference" "R1" …)` / `(property "Value" "10k" …)`。两种并存，优先 property。
 * - 图形描边：KiCad 6 的 gr_* 写 `(width …)`，KiCad 7/8 写 `(stroke (width …) …)`；
 *   板框只取坐标，不读描边，所以两代写法都能算。
 * - 焊盘网络一直是 `(net <code> "<name>")`；网名、层名不加引号是更早版本的写法，
 *   原子读取对引号/裸值一致，两种都收。
 *
 * 纯函数、零 IO。只有顶层 head 不是 `kicad_pcb` 时抛 `unsupported_format`
 * （KiCad 5 及更早的 `.brd` 不是 s-expression，parseSExpr 会先失败，这里统一改写成同一错误码）；
 * 其余识别不了的子结构一律忽略，不抛错。
 */

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
  type SExpr,
  type SExprList,
} from "./sexpr.js";

export interface KicadPcbDocument {
  /** 板框（Edge.Cuts 图形项的包围盒），无法计算时为 undefined */
  outline?: { width: number; height: number };
  nets: { number: number; name: string }[];
  footprints: KicadPcbFootprint[];
}

export interface KicadPcbFootprint {
  /** 元素 id：优先 (uuid "…")，其次 KiCad 6 的 (tstamp "…")，都没有时用 "pcb:<ref>@<x>,<y>" 这样的确定性回退 */
  id: string;
  ref: string;
  value?: string;
  footprint?: string;
  at: { x: number; y: number; rotation: number };
  layer?: string;
  pads: { number: string; netName?: string; netNumber?: number }[];
}

const EDGE_CUTS_LAYER = "Edge.Cuts";

export function parseKicadPcb(text: string): KicadPcbDocument {
  const root = parseBoardDocument(text);
  return {
    outline: readOutline(root),
    nets: readNets(root),
    footprints: childListsOf(root, "footprint").map(readFootprint),
  };
}

function parseBoardDocument(text: string): SExprList {
  let root: SExprList;
  try {
    root = parseSExpr(text);
  } catch (error) {
    // KiCad 5 及更早的 `.brd` 是另一套（非 s-expression）格式，解析器先失败；
    // 这里改写成 unsupported_format，调用方只需处理 §5 里列出的错误码。
    throw new Error(`unsupported_format: 不是 KiCad s-expression 文档：${errorMessage(error)}`);
  }
  const head = listHead(root);
  if (head !== "kicad_pcb") {
    throw new Error(`unsupported_format: 顶层 head 为 ${head ?? "(非列表)"}，期望 kicad_pcb`);
  }
  return root;
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** 顶层 `(net <code> "<name>")` 列表。按文件原样读出，连 code 0 / 空名字也不过滤。 */
function readNets(root: SExprList): { number: number; name: string }[] {
  const nets: { number: number; name: string }[] = [];
  for (const net of childListsOf(root, "net")) {
    const code = numberOf(net, 1);
    // 连编号都没有的 net 无法被焊盘引用，属于识别不了的结构，跳过。
    if (code === undefined) continue;
    nets.push({ number: code, name: atomAt(net, 2) ?? "" });
  }
  return nets;
}

function readFootprint(node: SExprList): KicadPcbFootprint {
  const properties = readProperties(node);
  const legacy = readLegacyTexts(node);
  // 位号是 IR 里组件的必需字段（docs/circuit/README.md §4）和修正的定位键，缺失时填空串保留元素，
  // 由上层作为 diagnostic 处理；不编造位号。
  const ref = firstNonEmpty(properties.get("Reference"), legacy.reference) ?? "";
  const value = firstNonEmpty(properties.get("Value"), legacy.value);
  // 封装名来自 `(footprint "Lib:Name" …)` 的第一个参数；KiCad 7/8 还冗余写一份 property "Footprint"。
  const footprint = firstNonEmpty(atomAt(node, 1), properties.get("Footprint"));
  const point = readPoint(firstChild(node, "at"));
  const at = { x: point?.x ?? 0, y: point?.y ?? 0, rotation: point?.rotation ?? 0 };
  const id = firstNonEmpty(childAtom(node, "uuid"), childAtom(node, "tstamp")) ?? `pcb:${ref}@${at.x},${at.y}`;
  return {
    id,
    ref,
    value,
    footprint,
    at,
    layer: nonEmpty(childAtom(node, "layer")),
    pads: childListsOf(node, "pad").map(readPad),
  };
}

/** KiCad 6 用 `(fp_text reference "R1" …)` / `(fp_text value "10k" …)` 表示位号与值；只取代换用户文本。 */
function readLegacyTexts(node: SExpr): { reference?: string; value?: string } {
  const texts: { reference?: string; value?: string } = {};
  for (const text of childListsOf(node, "fp_text")) {
    const kind = atomAt(text, 1)?.toLowerCase();
    const content = nonEmpty(atomAt(text, 2));
    if (kind === "reference" && texts.reference === undefined) texts.reference = content;
    if (kind === "value" && texts.value === undefined) texts.value = content;
  }
  return texts;
}

function readPad(node: SExprList): KicadPcbFootprint["pads"][number] {
  const net = firstChild(node, "net");
  return {
    // NPTH 机械孔会写成 `(pad "" np_thru_hole …)`，编号本来就是空串，按原样保留。
    number: atomAt(node, 1) ?? "",
    // `(net 3 "GND")`：编号取自数字、名字取自字符串；没有 (net …) 时两者都是 undefined。
    netName: net === undefined ? undefined : atomAt(net, 2),
    netNumber: numberOf(net, 1),
  };
}

/**
 * 板框：Edge.Cuts 图层上 gr_line/gr_rect/gr_arc/gr_circle 的包围盒，单位 mm。
 * gr_line/gr_rect 取 start/end，gr_arc 取 start/mid/end 三个控制点，
 * gr_circle 取圆心 ± 半径（半径由 center→end 两点算出）。
 * 没有任何 Edge.Cuts 图形项时返回 undefined，不编造尺寸。
 */
function readOutline(root: SExprList): { width: number; height: number } | undefined {
  const points: { x: number; y: number }[] = [];
  for (const item of childLists(root)) {
    const head = listHead(item);
    if (head !== "gr_line" && head !== "gr_rect" && head !== "gr_arc" && head !== "gr_circle") continue;
    if (childAtom(item, "layer") !== EDGE_CUTS_LAYER) continue;
    switch (head) {
      case "gr_line":
      case "gr_rect":
        pushPoint(points, firstChild(item, "start"));
        pushPoint(points, firstChild(item, "end"));
        break;
      case "gr_arc":
        pushPoint(points, firstChild(item, "start"));
        pushPoint(points, firstChild(item, "mid"));
        pushPoint(points, firstChild(item, "end"));
        break;
      case "gr_circle": {
        const center = readPoint(firstChild(item, "center"));
        const edge = readPoint(firstChild(item, "end"));
        if (center === undefined || edge === undefined) break;
        const radius = Math.hypot(edge.x - center.x, edge.y - center.y);
        points.push({ x: center.x - radius, y: center.y - radius });
        points.push({ x: center.x + radius, y: center.y + radius });
        break;
      }
    }
  }
  const first = points[0];
  if (first === undefined) return undefined;
  let minX = first.x;
  let minY = first.y;
  let maxX = first.x;
  let maxY = first.y;
  for (const point of points) {
    minX = Math.min(minX, point.x);
    minY = Math.min(minY, point.y);
    maxX = Math.max(maxX, point.x);
    maxY = Math.max(maxY, point.y);
  }
  return { width: maxX - minX, height: maxY - minY };
}

function pushPoint(points: { x: number; y: number }[], node: SExpr | undefined): void {
  const point = readPoint(node);
  if (point !== undefined) points.push({ x: point.x, y: point.y });
}

/** 读取 `(head "值")` 形式的直接子节点，例如 `(layer "F.Cu")`、`(uuid "…")`。 */
function childAtom(node: SExpr, head: string): string | undefined {
  const child = firstChild(node, head);
  return child === undefined ? undefined : atomAt(child, 1);
}

/** KiCad 会把没有值的字段写成空串（如 `(property "Value" "")`）；空串按“没有该字段”处理。 */
function nonEmpty(value: string | undefined): string | undefined {
  return value !== undefined && value.length > 0 ? value : undefined;
}

function firstNonEmpty(...values: (string | undefined)[]): string | undefined {
  return values.find((value) => value !== undefined && value.length > 0);
}
