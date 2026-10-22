import type { SvnBranchGraphEdge, SvnBranchGraphNode, SvnBranchGraphResult } from "@escode/shared";

export interface SvnGraphLayoutNode {
  node: SvnBranchGraphNode;
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface SvnGraphLayoutEdge {
  edge: SvnBranchGraphEdge;
  path: string;
  labelX: number;
  labelY: number;
}

export interface SvnBranchGraphLayout {
  width: number;
  height: number;
  nodes: SvnGraphLayoutNode[];
  edges: SvnGraphLayoutEdge[];
}

const GRAPH_NODE_WIDTH = 176;
const GRAPH_NODE_HEIGHT = 42;
const GRAPH_NODE_GAP = 28;
const GRAPH_PADDING = 24;
const GRAPH_ROW_GAP = 116;

/**
 * 固定分层布局：trunk / branches / tags / other 各一行，trunk 居中。
 * 只负责几何计算，便于单测；渲染与文案在 `SvnBranchGraphDialog`。
 */
export function layoutSvnBranchGraph(graph: SvnBranchGraphResult): SvnBranchGraphLayout {
  const rows: Record<"trunk" | "branch" | "tag" | "other", SvnBranchGraphNode[]> = {
    trunk: [],
    branch: [],
    tag: [],
    other: [],
  };
  for (const node of graph.nodes) {
    if (node.kind === "trunk") {
      rows.trunk.push(node);
    } else if (node.kind === "branch") {
      rows.branch.push(node);
    } else if (node.kind === "tag") {
      rows.tag.push(node);
    } else {
      rows.other.push(node);
    }
  }
  const rowEntries = (
    Object.entries(rows) as Array<[keyof typeof rows, SvnBranchGraphNode[]]>
  ).filter(([, nodes]) => nodes.length > 0);
  const maxRowCount = Math.max(1, ...rowEntries.map(([, nodes]) => nodes.length));
  const width = Math.max(
    360,
    GRAPH_PADDING * 2 + maxRowCount * GRAPH_NODE_WIDTH + (maxRowCount - 1) * GRAPH_NODE_GAP,
  );
  const nodes: SvnGraphLayoutNode[] = [];
  const byPath = new Map<string, SvnGraphLayoutNode>();

  rowEntries.forEach(([row, rowNodes], rowIndex) => {
    const y = GRAPH_PADDING + rowIndex * GRAPH_ROW_GAP;
    rowNodes.forEach((node, index) => {
      const x =
        row === "trunk"
          ? (width - GRAPH_NODE_WIDTH) / 2
          : GRAPH_PADDING + index * (GRAPH_NODE_WIDTH + GRAPH_NODE_GAP);
      const positioned = {
        node,
        x: Math.min(x, width - GRAPH_NODE_WIDTH - GRAPH_PADDING),
        y,
        width: GRAPH_NODE_WIDTH,
        height: GRAPH_NODE_HEIGHT,
      };
      byPath.set(node.relativePath, positioned);
      nodes.push(positioned);
    });
  });

  const edges: SvnGraphLayoutEdge[] = [];
  graph.edges.forEach((edge, index) => {
    const from = byPath.get(edge.from);
    const to = byPath.get(edge.to);
    if (!from || !to) {
      return;
    }
    const fromX = from.x + from.width / 2;
    const fromY = from.y + from.height / 2;
    const toX = to.x + to.width / 2;
    const toY = to.y + to.height / 2;
    const bend = 46 + (index % 3) * 16;
    const path =
      edge.kind === "copy"
        ? `M ${fromX} ${from.y + from.height} C ${fromX} ${fromY + bend}, ${toX} ${toY - bend}, ${toX} ${to.y}`
        : fromY > toY
          ? `M ${fromX} ${from.y} C ${fromX - bend} ${fromY - bend}, ${toX + bend} ${toY + bend}, ${toX} ${to.y + to.height}`
          : `M ${fromX} ${from.y + from.height} C ${fromX} ${fromY + bend}, ${toX} ${toY - bend}, ${toX} ${to.y}`;
    edges.push({
      edge,
      path,
      labelX: (fromX + toX) / 2,
      labelY: (fromY + toY) / 2 - 6,
    });
  });

  const height =
    GRAPH_PADDING * 2 +
    (rowEntries.length > 0 ? (rowEntries.length - 1) * GRAPH_ROW_GAP : 0) +
    GRAPH_NODE_HEIGHT;
  return { width, height, nodes, edges };
}
