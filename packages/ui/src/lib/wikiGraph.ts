// 知识图谱页的纯逻辑（apps/escode-cli-rust/docs/llm-wiki/01-spec.md §11）：布局、配色语义与可用操作。
// 不持有知识事实：输入是 runtime 返回的 nodes/edges 快照，输出只用于渲染。
import type {
  V4WikiGraphEdge,
  V4WikiGraphNode,
  WikiKind,
  WikiStatus,
} from "@escode/shared/escode-protocol-v4";

export interface WikiNodePosition {
  x: number;
  y: number;
}

export interface WikiLayoutOptions {
  width?: number;
  height?: number;
  /** 缺省按节点数自适应：节点越多迭代越少，保证 2000 节点上限内仍可交互。 */
  iterations?: number;
  /** 标签包围盒（像素），用于布局后的防重叠；默认与节点标签 max-w-40/行高相当。 */
  labelWidth?: number;
  labelHeight?: number;
}

/** FNV-1a：同一 id 每次得到同一初始位置，刷新后布局不跳动。 */
function hash(value: string): number {
  let h = 0x811c9dc5;
  for (let i = 0; i < value.length; i += 1) {
    h ^= value.charCodeAt(i);
    h = Math.imul(h, 0x01000193);
  }
  return h >>> 0;
}

function seeded(id: string, width: number, height: number): WikiNodePosition {
  const h = hash(id);
  return { x: ((h & 0xffff) / 0xffff) * width, y: (((h >>> 16) & 0xffff) / 0xffff) * height };
}

function defaultIterations(count: number): number {
  if (count <= 1) return 0;
  return Math.max(40, Math.min(300, Math.floor(120_000 / (count * count)) + 40));
}

/**
 * 确定性的 Fruchterman–Reingold 力导向布局：节点互斥、边相吸、温度线性冷却。
 * 只依赖 id 与边，不使用随机数，便于测试与刷新稳定。
 */
export function layoutWikiGraph(
  nodes: readonly Pick<V4WikiGraphNode, "id">[],
  edges: readonly Pick<V4WikiGraphEdge, "from" | "to">[],
  options: WikiLayoutOptions = {},
): Map<string, WikiNodePosition> {
  const count = nodes.length;
  const width = options.width ?? Math.max(600, Math.sqrt(count) * 160);
  const height = options.height ?? width * 0.7;
  const positions = nodes.map((node) => seeded(node.id, width, height));
  const indexById = new Map(nodes.map((node, index) => [node.id, index]));
  const links = edges
    .map((edge) => [indexById.get(edge.from), indexById.get(edge.to)] as const)
    .filter(
      (pair): pair is readonly [number, number] =>
        pair[0] !== undefined && pair[1] !== undefined && pair[0] !== pair[1],
    );
  const k = Math.sqrt((width * height) / Math.max(count, 1)) * 0.75;
  const iterations = options.iterations ?? defaultIterations(count);
  const dx = new Float64Array(count);
  const dy = new Float64Array(count);
  for (let step = 0; step < iterations; step += 1) {
    dx.fill(0);
    dy.fill(0);
    for (let i = 0; i < count; i += 1) {
      for (let j = i + 1; j < count; j += 1) {
        let ox = positions[i]!.x - positions[j]!.x;
        let oy = positions[i]!.y - positions[j]!.y;
        let distance = Math.hypot(ox, oy);
        if (distance < 0.01) {
          // 完全重合时按序号给一个确定方向，避免除零。
          ox = ((i - j) % 7) + 0.5;
          oy = ((j - i) % 5) + 0.5;
          distance = Math.hypot(ox, oy);
        }
        const force = (k * k) / distance;
        dx[i]! += (ox / distance) * force;
        dy[i]! += (oy / distance) * force;
        dx[j]! -= (ox / distance) * force;
        dy[j]! -= (oy / distance) * force;
      }
    }
    for (const [a, b] of links) {
      const ox = positions[a]!.x - positions[b]!.x;
      const oy = positions[a]!.y - positions[b]!.y;
      const distance = Math.max(Math.hypot(ox, oy), 0.01);
      const force = (distance * distance) / k;
      dx[a]! -= (ox / distance) * force;
      dy[a]! -= (oy / distance) * force;
      dx[b]! += (ox / distance) * force;
      dy[b]! += (oy / distance) * force;
    }
    const temperature = (width / 10) * (1 - step / iterations);
    for (let i = 0; i < count; i += 1) {
      const length = Math.hypot(dx[i]!, dy[i]!);
      if (length === 0) continue;
      const move = Math.min(length, temperature);
      const next = positions[i]!;
      next.x = Math.min(width, Math.max(0, next.x + (dx[i]! / length) * move));
      next.y = Math.min(height, Math.max(0, next.y + (dy[i]! / length) * move));
    }
  }
  resolveLabelCollisions(positions, options.labelWidth ?? 120, options.labelHeight ?? 36);
  return new Map(nodes.map((node, index) => [node.id, positions[index]!]));
}

/**
 * 力导向只保证点不重合；共享同一来源的条目会挤成一团、标签互相覆盖。
 * 这里按标签包围盒做确定性分离：重叠的两点沿重叠较小的轴各退一半，允许超出画布（视图 fitView 自适应）。
 */
function resolveLabelCollisions(
  positions: WikiNodePosition[],
  labelWidth: number,
  labelHeight: number,
) {
  const passes = positions.length > 800 ? 8 : 24;
  for (let pass = 0; pass < passes; pass += 1) {
    let moved = false;
    for (let i = 0; i < positions.length; i += 1) {
      for (let j = i + 1; j < positions.length; j += 1) {
        const a = positions[i]!;
        const b = positions[j]!;
        const dx = b.x - a.x;
        const dy = b.y - a.y;
        const overlapX = labelWidth - Math.abs(dx);
        const overlapY = labelHeight - Math.abs(dy);
        if (overlapX <= 0 || overlapY <= 0) continue;
        moved = true;
        // 完全同坐标时用序号决定方向，保证结果可复现。
        if (overlapY <= overlapX) {
          const shift = (overlapY / 2) * (dy === 0 ? (i % 2 === 0 ? 1 : -1) : Math.sign(dy));
          a.y -= shift;
          b.y += shift;
        } else {
          const shift = (overlapX / 2) * (dx === 0 ? 1 : Math.sign(dx));
          a.x -= shift;
          b.x += shift;
        }
      }
    }
    if (!moved) return;
  }
}

/** 节点颜色只表达 kind 分组（spec §11：颜色=kind）；状态由描边/透明度表达。 */
export type WikiKindTone =
  | "brand"
  | "blue"
  | "purple"
  | "success"
  | "warning"
  | "destructive"
  | "neutral";

const KIND_TONE: Record<WikiKind, WikiKindTone> = {
  standard: "brand",
  "business-rule": "brand",
  invariant: "brand",
  architecture: "blue",
  adr: "blue",
  contract: "blue",
  guide: "success",
  reference: "purple",
  pattern: "success",
  "anti-pattern": "destructive",
  proposal: "warning",
};

export function wikiKindTone(kind: WikiKind | undefined): WikiKindTone {
  return kind ? KIND_TONE[kind] : "neutral";
}

export const WIKI_TONE_CLASS: Record<WikiKindTone, string> = {
  brand: "bg-brand",
  blue: "bg-icon-blue",
  purple: "bg-icon-purple",
  success: "bg-success",
  warning: "bg-warning",
  destructive: "bg-destructive",
  neutral: "bg-foreground-subtlest",
};

/** 状态呈现：approved 实线；proposal 虚线；needs-review 警示描边；退役状态淡化。 */
export function wikiStatusPresentation(status: WikiStatus | undefined): {
  border: "solid" | "dashed" | "warning";
  faded: boolean;
} {
  switch (status) {
    case "proposal":
      return { border: "dashed", faded: false };
    case "needs-review":
      return { border: "warning", faded: false };
    case "deprecated":
    case "superseded":
    case "rejected":
      return { border: "solid", faded: true };
    default:
      return { border: "solid", faded: false };
  }
}

export function wikiEdgeStyle(type: V4WikiGraphEdge["type"]): {
  stroke: string;
  strokeDasharray?: string;
  animated: boolean;
} {
  switch (type) {
    case "conflict":
      return { stroke: "var(--color-destructive)", strokeDasharray: "4 3", animated: false };
    case "supersedes":
      return { stroke: "var(--color-foreground-subtle)", strokeDasharray: "6 4", animated: false };
    case "source":
      return { stroke: "var(--color-border-hover)", animated: false };
    default:
      return { stroke: "var(--color-foreground-subtlest)", animated: false };
  }
}

/** 用户可对条目发起的治理动作（spec §1：只有用户显式操作才能改变规范状态）。 */
export type WikiReviewAction = "approve" | "reject" | "deprecate";

export function wikiReviewActions(status: WikiStatus): WikiReviewAction[] {
  switch (status) {
    case "proposal":
      return ["approve", "reject"];
    case "needs-review":
      return ["approve", "deprecate"];
    case "approved":
      return ["deprecate"];
    default:
      return [];
  }
}

/** 节点度数，用于详情中的关联数量与图上节点尺寸。 */
export function wikiNodeDegrees(
  edges: readonly Pick<V4WikiGraphEdge, "from" | "to">[],
): Map<string, number> {
  const degrees = new Map<string, number>();
  for (const edge of edges) {
    degrees.set(edge.from, (degrees.get(edge.from) ?? 0) + 1);
    degrees.set(edge.to, (degrees.get(edge.to) ?? 0) + 1);
  }
  return degrees;
}

/** 选中节点的一跳邻居（高亮用）。 */
export function wikiNeighborhood(
  selectedId: string | null,
  edges: readonly Pick<V4WikiGraphEdge, "from" | "to">[],
): Set<string> {
  const result = new Set<string>();
  if (!selectedId) return result;
  result.add(selectedId);
  for (const edge of edges) {
    if (edge.from === selectedId) result.add(edge.to);
    if (edge.to === selectedId) result.add(edge.from);
  }
  return result;
}
