import { isLowConfidence, type CircuitComponent, type CircuitNet, type CircuitOrigin } from "@escode/shared/circuit";
import type { CircuitLayoutSheet, CircuitLayoutSymbol } from "@escode/services";

/**
 * 面板的纯视图计算（规格 §8）：低置信度标记、BOM/网络表标签、选中后的连接高亮范围、
 * 以及 SVG 需要的几何派生值。这里不碰服务，也不持有 IR 事实——入参都是 hooks 读到的 IR/layout 快照。
 */

/** 引脚坐标的比较精度（图纸单位 mm，文本里的同一点位是同一个字面量，1e-4 足够）。 */
const POINT_EPSILON = 1e-4;

export interface CircuitSelection {
  componentId?: string;
  netId?: string;
  /** 当前图纸实例路径；空串是根图纸。 */
  sheetPath: string;
}

export interface CircuitHighlight {
  componentIds: ReadonlySet<string>;
  netIds: ReadonlySet<string>;
  /** `${componentId}\0${pin}`：要加粗的引脚。 */
  pins: ReadonlySet<string>;
}

export function pinKey(componentId: string, pin: string): string {
  return `${componentId}\u0000${pin}`;
}

export function isLowConfidenceOrigin(origin: CircuitOrigin): boolean {
  return isLowConfidence(origin);
}

/** 低置信度元素集合：器件与网络（§2 规则 4「低置信度必须可见」）。 */
export function collectLowConfidenceIds(
  components: readonly CircuitComponent[],
  nets: readonly CircuitNet[],
): ReadonlySet<string> {
  const ids = new Set<string>();
  for (const component of components) if (isLowConfidence(component.origin)) ids.add(component.id);
  for (const net of nets) if (isLowConfidence(net.origin)) ids.add(net.id);
  return ids;
}

export function netIdsOfComponent(component: CircuitComponent): string[] {
  const ids = new Set<string>();
  for (const pin of component.pins) if (pin.netId) ids.add(pin.netId);
  return [...ids];
}

export function componentIdsOfNet(net: CircuitNet): string[] {
  const ids = new Set<string>();
  for (const pinRef of net.pinRefs) ids.add(pinRef.componentId);
  return [...ids];
}

export function netLabel(net: CircuitNet): string {
  return net.name ?? net.id;
}

export function componentLabel(component: CircuitComponent): string {
  return component.value ? `${component.ref} · ${component.value}` : component.ref;
}

/** 引脚展示名：有名字时 `1 (VCC)`，没有就只给编号。 */
export function pinLabel(pin: { number: string; name?: string }): string {
  return pin.name ? `${pin.number} (${pin.name})` : pin.number;
}

/**
 * 选中一个器件或一条网络时要高亮的元素（§8「点击器件或网络，高亮整条连接」）：
 * 网络由 IR 的 pinRefs 决定；器件高亮它所有引脚所在的网络，网络高亮它全部引脚。
 */
export function buildHighlight(
  selection: CircuitSelection,
  components: readonly CircuitComponent[],
  nets: readonly CircuitNet[],
): CircuitHighlight {
  const componentById = new Map(components.map((component) => [component.id, component]));
  const netById = new Map(nets.map((net) => [net.id, net]));
  const netIds = new Set<string>();
  const componentIds = new Set<string>();
  const pins = new Set<string>();
  if (selection.componentId) {
    const component = componentById.get(selection.componentId);
    if (component) {
      componentIds.add(component.id);
      for (const netId of netIdsOfComponent(component)) netIds.add(netId);
    }
  }
  if (selection.netId) {
    const net = netById.get(selection.netId);
    if (net) {
      netIds.add(net.id);
      for (const pinRef of net.pinRefs) {
        componentIds.add(pinRef.componentId);
        pins.add(pinKey(pinRef.componentId, pinRef.pin));
      }
    }
  }
  if (selection.componentId && !selection.netId) {
    // 器件选中：同一网络上的每个引脚都算连接点，侧栏与 SVG 用同一份集合。
    for (const netId of netIds) {
      const net = netById.get(netId);
      if (!net) continue;
      for (const pinRef of net.pinRefs) {
        componentIds.add(pinRef.componentId);
        pins.add(pinKey(pinRef.componentId, pinRef.pin));
      }
    }
  }
  return { componentIds, netIds, pins };
}

export function samePoint(
  left: { x: number; y: number },
  right: { x: number; y: number },
): boolean {
  return Math.abs(left.x - right.x) <= POINT_EPSILON && Math.abs(left.y - right.y) <= POINT_EPSILON;
}

/** 高亮引脚在图纸上的连接点（连接点与网络推导用的点一致，见 CircuitLayoutPin）。 */
export function highlightedPinPoints(
  sheet: CircuitLayoutSheet,
  highlight: CircuitHighlight,
): { x: number; y: number }[] {
  const points: { x: number; y: number }[] = [];
  for (const symbol of sheet.symbols) {
    if (!symbol.elementId) continue;
    for (const pin of symbol.pins)
      if (highlight.pins.has(pinKey(symbol.elementId, pin.number))) points.push(pin.connection);
  }
  return points;
}

/**
 * 与高亮引脚相连的导线：layout 的 wire 不携带网络 id，只有几何，
 * 因此按「顶点落在连接点上」判定（同一次解析产出，点位来自同一份源文件字面量）。
 */
export function wireTouchesPoints(
  wire: { points: { x: number; y: number }[] },
  points: readonly { x: number; y: number }[],
): boolean {
  if (points.length === 0) return false;
  return wire.points.some((vertex) => points.some((point) => samePoint(vertex, point)));
}

export function isSymbolHighlighted(
  symbol: CircuitLayoutSymbol,
  highlight: CircuitHighlight,
): boolean {
  return symbol.elementId !== undefined && highlight.componentIds.has(symbol.elementId);
}

export interface LayoutViewBox {
  minX: number;
  minY: number;
  width: number;
  height: number;
}

/** SVG viewBox：图纸尺寸与内容外接矩形的并集，四周留 padding（mm）。 */
export function layoutViewBox(sheet: CircuitLayoutSheet, padding = 4): LayoutViewBox {
  let minX = 0;
  let minY = 0;
  let maxX = sheet.page.width;
  let maxY = sheet.page.height;
  const extend = (x: number, y: number) => {
    minX = Math.min(minX, x);
    minY = Math.min(minY, y);
    maxX = Math.max(maxX, x);
    maxY = Math.max(maxY, y);
  };
  for (const symbol of sheet.symbols) {
    extend(symbol.x, symbol.y);
    for (const shape of symbol.shapes) for (const point of shape.points) extend(point.x, point.y);
    for (const pin of symbol.pins) {
      extend(pin.connection.x, pin.connection.y);
      extend(pin.body.x, pin.body.y);
    }
    for (const text of symbol.texts) extend(text.x, text.y);
  }
  for (const wire of sheet.wires) for (const point of wire.points) extend(point.x, point.y);
  for (const junction of sheet.junctions) extend(junction.x, junction.y);
  for (const label of sheet.labels) extend(label.x, label.y);
  const width = Math.max(maxX - minX, 1) + padding * 2;
  const height = Math.max(maxY - minY, 1) + padding * 2;
  return { minX: minX - padding, minY: minY - padding, width, height };
}

/** 圆弧（起 / 中 / 止）→ SVG path；三点共线时退化成折线。 */
export function arcPath(points: readonly { x: number; y: number }[]): string {
  const [start, middle, end] = points;
  if (!start || !middle || !end)
    return points.map((point, index) => `${index === 0 ? "M" : "L"}${point.x} ${point.y}`).join(" ");
  const determinant =
    2 * (start.x * (middle.y - end.y) + middle.x * (end.y - start.y) + end.x * (start.y - middle.y));
  if (Math.abs(determinant) < 1e-9)
    return `M${start.x} ${start.y} L${middle.x} ${middle.y} L${end.x} ${end.y}`;
  const startSquared = start.x * start.x + start.y * start.y;
  const middleSquared = middle.x * middle.x + middle.y * middle.y;
  const endSquared = end.x * end.x + end.y * end.y;
  const centerX =
    (startSquared * (middle.y - end.y) +
      middleSquared * (end.y - start.y) +
      endSquared * (start.y - middle.y)) /
    determinant;
  const centerY =
    (startSquared * (end.x - middle.x) +
      middleSquared * (start.x - end.x) +
      endSquared * (middle.x - start.x)) /
    determinant;
  const radius = Math.hypot(start.x - centerX, start.y - centerY);
  const startAngle = Math.atan2(start.y - centerY, start.x - centerX);
  const middleAngle = Math.atan2(middle.y - centerY, middle.x - centerX);
  const endAngle = Math.atan2(end.y - centerY, end.x - centerX);
  const normalize = (angle: number) => ((angle % (Math.PI * 2)) + Math.PI * 2) % (Math.PI * 2);
  const sweepForward = normalize(middleAngle - startAngle) < normalize(endAngle - startAngle);
  const largeArc = normalize(endAngle - startAngle) > Math.PI ? 1 : 0;
  return `M${start.x} ${start.y} A${radius} ${radius} 0 ${largeArc} ${sweepForward ? 1 : 0} ${end.x} ${end.y}`;
}

export function polylinePoints(points: readonly { x: number; y: number }[]): string {
  return points.map((point) => `${point.x},${point.y}`).join(" ");
}
