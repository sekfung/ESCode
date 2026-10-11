import type { KicadPoint, KicadWire } from "./kicadSchematic.js";

/**
 * 单页连通性推导：把导线、结点、引脚、标签按几何关系聚成网络。
 * 这是 KiCad 网表规则的确定性子集，用于 `.kicad_sch`（原理图文件里没有显式网表）：
 *
 * - 两条导线共享端点 → 连通。
 * - 导线端点落在另一条导线中间 → 连通（KiCad 会在此自动生成结点）。
 * - 两条导线只是中间十字交叉且没有结点 → **不**连通。
 * - 有结点的位置：经过该点的所有导线互相连通（端点在中间也算）。
 * - 引脚 / 标签 / 图纸引脚落在导线上（端点或中间）→ 与该导线连通；同一位置彼此也连通。
 *
 * 总线（bus）不参与推导：总线成员通常也会以标签形式出现在分支导线上，这些标签照常成网。
 */

/** 连接判定容差（mm）：KiCad 坐标写到 0.0001 mm，容差取同一量级。 */
export const CONNECT_EPSILON = 1e-4;

/** 导线空间索引的格边长（mm），只用来缩小几何判定的候选集合。 */
const INDEX_CELL_MM = 1;

export type TerminalKind =
  | "pin"
  | "label"
  | "global-label"
  | "hierarchical-label"
  | "power"
  | "sheet-pin";

export interface PageTerminal {
  kind: TerminalKind;
  /** 标签文本 / 电源网络名 / 图纸引脚名。 */
  name?: string;
  point: KicadPoint;
  /** 引脚所属器件（同一器件跨单元合并后由上层给出）。 */
  componentKey?: string;
  pinNumber?: string;
}

export interface PagePinRef {
  componentKey: string;
  pinNumber: string;
}

export interface PageNet {
  terminals: PageTerminal[];
  pins: PagePinRef[];
}

export interface PageConnectivity {
  nets: PageNet[];
}

/** 并查集（字符串键）：页内连通性与跨页网络合并共用同一个实现。 */
export class UnionFind {
  private readonly parents = new Map<string, string>();

  add(key: string): void {
    if (!this.parents.has(key)) this.parents.set(key, key);
  }

  find(key: string): string {
    const parent = this.parents.get(key);
    if (parent === undefined || parent === key) return key;
    const root = this.find(parent);
    this.parents.set(key, root);
    return root;
  }

  union(a: string, b: string): void {
    this.add(a);
    this.add(b);
    const rootA = this.find(a);
    const rootB = this.find(b);
    if (rootA !== rootB) this.parents.set(rootB, rootA);
  }
}

interface PointBucket {
  /** 以该点为端点的导线段下标。 */
  endpoints: number[];
  /** 经过该点的导线段下标（该点是线段内部点）。 */
  crossings: number[];
  junctionCount: number;
  /** 以该点为位置的终端下标。 */
  terminals: number[];
}

function pointKey(point: KicadPoint): string {
  return `${Math.round(point.x / CONNECT_EPSILON)}:${Math.round(point.y / CONNECT_EPSILON)}`;
}

function parsePointKey(key: string): KicadPoint {
  const [x, y] = key.split(":").map(Number) as [number, number];
  return { x: x * CONNECT_EPSILON, y: y * CONNECT_EPSILON };
}

/** 点是否落在线段上（含端点）：叉积判定共线 + 包围盒判定落在段内。 */
export function isOnSegment(point: KicadPoint, from: KicadPoint, to: KicadPoint): boolean {
  const cross = (to.x - from.x) * (point.y - from.y) - (to.y - from.y) * (point.x - from.x);
  if (Math.abs(cross) > CONNECT_EPSILON) return false;
  return (
    point.x >= Math.min(from.x, to.x) - CONNECT_EPSILON &&
    point.x <= Math.max(from.x, to.x) + CONNECT_EPSILON &&
    point.y >= Math.min(from.y, to.y) - CONNECT_EPSILON &&
    point.y <= Math.max(from.y, to.y) + CONNECT_EPSILON
  );
}

export function computePageNets(input: {
  wires: KicadWire[];
  junctions: KicadPoint[];
  terminals: PageTerminal[];
}): PageConnectivity {
  const segments: { from: KicadPoint; to: KicadPoint }[] = [];
  for (const wire of input.wires) {
    for (let index = 1; index < wire.points.length; index += 1)
      segments.push({ from: wire.points[index - 1]!, to: wire.points[index]! });
  }
  const segmentKeys = segments.map((_, index) => `segment:${index}`);
  const terminalKeys = input.terminals.map((_, index) => `terminal:${index}`);

  const uf = new UnionFind();
  for (const key of segmentKeys) uf.add(key);
  for (const key of terminalKeys) uf.add(key);

  const buckets = new Map<string, PointBucket>();
  const bucketAt = (point: KicadPoint): PointBucket => {
    const key = pointKey(point);
    const existing = buckets.get(key);
    if (existing) return existing;
    const created: PointBucket = { endpoints: [], crossings: [], junctionCount: 0, terminals: [] };
    buckets.set(key, created);
    return created;
  };
  for (const [index, segment] of segments.entries()) {
    bucketAt(segment.from).endpoints.push(index);
    bucketAt(segment.to).endpoints.push(index);
  }
  for (const junction of input.junctions) bucketAt(junction).junctionCount += 1;
  for (const [index, terminal] of input.terminals.entries())
    bucketAt(terminal.point).terminals.push(index);

  // 端点落在其它导线中间的情况：按格点索引只检查与线段包围盒相交的候选点。
  const cells = new Map<string, string[]>();
  for (const key of buckets.keys()) {
    const point = parsePointKey(key);
    const cellKey = `${Math.floor(point.x / INDEX_CELL_MM)}:${Math.floor(point.y / INDEX_CELL_MM)}`;
    const list = cells.get(cellKey);
    if (list) list.push(key);
    else cells.set(cellKey, [key]);
  }
  for (const [index, segment] of segments.entries()) {
    const minX = Math.floor(Math.min(segment.from.x, segment.to.x) / INDEX_CELL_MM);
    const maxX = Math.floor(Math.max(segment.from.x, segment.to.x) / INDEX_CELL_MM);
    const minY = Math.floor(Math.min(segment.from.y, segment.to.y) / INDEX_CELL_MM);
    const maxY = Math.floor(Math.max(segment.from.y, segment.to.y) / INDEX_CELL_MM);
    for (let cellX = minX - 1; cellX <= maxX + 1; cellX += 1) {
      for (let cellY = minY - 1; cellY <= maxY + 1; cellY += 1) {
        for (const key of cells.get(`${cellX}:${cellY}`) ?? []) {
          const bucket = buckets.get(key)!;
          if (bucket.endpoints.includes(index) || bucket.crossings.includes(index)) continue;
          if (isOnSegment(parsePointKey(key), segment.from, segment.to))
            bucket.crossings.push(index);
        }
      }
    }
  }

  for (const bucket of buckets.values()) {
    const bridge = bucket.junctionCount > 0 || bucket.terminals.length > 0;
    const connected = bridge ? [...bucket.endpoints, ...bucket.crossings] : bucket.endpoints;
    const first = connected[0];
    for (let index = 1; index < connected.length; index += 1)
      if (first !== undefined) uf.union(segmentKeys[first]!, segmentKeys[connected[index]!]!);
    for (const terminalIndex of bucket.terminals) {
      if (first !== undefined) uf.union(terminalKeys[terminalIndex]!, segmentKeys[first]!);
      else
        for (const other of bucket.terminals)
          uf.union(terminalKeys[terminalIndex]!, terminalKeys[other]!);
    }
  }

  const nets: PageNet[] = [];
  const netIndexByRoot = new Map<string, number>();
  const netFor = (key: string): PageNet => {
    const root = uf.find(key);
    const existing = netIndexByRoot.get(root);
    if (existing !== undefined) return nets[existing]!;
    const created: PageNet = { terminals: [], pins: [] };
    nets.push(created);
    netIndexByRoot.set(root, nets.length - 1);
    return created;
  };
  for (const [index, terminal] of input.terminals.entries())
    netFor(terminalKeys[index]!).terminals.push(terminal);
  for (const net of nets)
    net.pins = net.terminals
      .filter((terminal) => terminal.kind === "pin" && terminal.componentKey && terminal.pinNumber)
      .map((terminal) => ({ componentKey: terminal.componentKey!, pinNumber: terminal.pinNumber! }));
  return { nets: nets.filter((net) => net.pins.length > 0 || net.terminals.some((t) => t.name)) };
}

/** 供上层做跨页合并时按名字取页内网络。 */
export function findPageNetByName(
  connectivity: PageConnectivity,
  kinds: TerminalKind[],
  name: string,
): PageNet | undefined {
  return connectivity.nets.find((net) =>
    net.terminals.some((terminal) => kinds.includes(terminal.kind) && terminal.name === name),
  );
}
