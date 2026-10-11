import {
  atomAt,
  childListsOf,
  firstChild,
  listHead,
  numberOf,
  readPoint,
  type SExprList,
} from "./sexpr.js";

/**
 * 库符号几何项的读取（`lib_symbols` 里的 rectangle/polyline/circle/arc 与引脚长度）。
 * 单位 mm、库坐标 +Y 向上；这里只做「节点 → 数值」，坐标变换统一由 kicadSchematic.transformSymbolPoint 负责。
 * 与 kicadSchematic.ts 分开是为了守住单文件行数上限，不改变模型语义。
 */

/** 库符号图形项（库坐标，单位 mm）。 */
export interface KicadLibGraphic {
  /** 来自子符号名 `…_<unit>_<bodystyle>`，0 = 所有单元共用。 */
  unit: number;
  kind: "rectangle" | "polyline" | "circle" | "arc";
  /** rectangle 取 start/end 两点；polyline 取全部 xy；circle 取圆心 + 圆上一点；arc 取 start/mid/end。 */
  points: { x: number; y: number }[];
  /** `(fill (type none|outline|background))`：type 非 none 即 true。 */
  filled: boolean;
}

/** 引脚长度（mm）：从连接点沿 `(at … rot)` 方向画进符号体；没有 `(length …)` 时按 0。 */
export function readPinLength(pin: SExprList): number {
  return numberOf(firstChild(pin, "length"), 1) ?? 0;
}

/** 图形控制点名：矩形取对角两点、弧取起/中/止、圆取圆心（半径点另算）；折线在 `(pts …)` 里。 */
const GRAPHIC_CONTROL_HEADS: Record<KicadLibGraphic["kind"], readonly string[]> = {
  rectangle: ["start", "end"],
  polyline: [],
  circle: ["center"],
  arc: ["start", "mid", "end"],
};

/**
 * 库符号图形项：只认面板能直接画的 rectangle/polyline/circle/arc，其余（bezier、text、pin 等）返回空。
 * circle 在库里写作 `(center x y) (radius r)`，按契约「圆心 + 圆上一点」合成 (x+r, y)。
 */
export function readLibGraphics(node: SExprList, unit: number): KicadLibGraphic[] {
  const kind = listHead(node);
  if (kind !== "rectangle" && kind !== "polyline" && kind !== "circle" && kind !== "arc")
    return [];
  const points = readGraphicPoints(kind, node);
  return points.length === 0 ? [] : [{ unit, kind, points, filled: isFilled(node) }];
}

function readGraphicPoints(kind: KicadLibGraphic["kind"], node: SExprList): { x: number; y: number }[] {
  const nodes =
    kind === "polyline"
      ? childListsOf(firstChild(node, "pts") ?? node, "xy")
      : GRAPHIC_CONTROL_HEADS[kind].map((head) => firstChild(node, head));
  const points = nodes.flatMap((child) => {
    const point = readPoint(child);
    return point === undefined ? [] : [{ x: point.x, y: point.y }];
  });
  if (kind !== "circle") return points;
  const center = points[0];
  const radius = numberOf(firstChild(node, "radius"), 1);
  return center === undefined || radius === undefined
    ? []
    : [center, { x: center.x + radius, y: center.y }];
}

/** `(fill (type none|outline|background))`：非 none 视为填充（outline 描边填充、background 背景填充）。 */
function isFilled(node: SExprList): boolean {
  const fill = firstChild(node, "fill");
  if (!fill) return false;
  const type = atomAt(firstChild(fill, "type") ?? fill, 1);
  return type !== undefined && type !== "none";
}
