// 知识图谱画布：力导向布局（lib/wikiGraph.ts）+ xyflow 提供平移/缩放/触控。
// 颜色=kind，描边/透明度=status，source 节点为方块（spec §11）。
import { memo, useMemo } from "react";
import {
  Handle,
  Position,
  ReactFlow,
  ReactFlowProvider,
  type Edge,
  type Node,
  type NodeProps,
} from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import type { V4WikiGraphEdge, V4WikiGraphNode } from "@escode/shared/escode-protocol-v4";
import { cn } from "@/components/lib/utils.js";
import { Controls } from "@/components/ai-elements/controls.js";
import {
  WIKI_TONE_CLASS,
  layoutWikiGraph,
  wikiEdgeStyle,
  wikiKindTone,
  wikiNeighborhood,
  wikiNodeDegrees,
  wikiStatusPresentation,
} from "@/lib/wikiGraph.js";

interface WikiNodeData extends Record<string, unknown> {
  node: V4WikiGraphNode;
  degree: number;
  selected: boolean;
  dimmed: boolean;
}

type WikiFlowNode = Node<WikiNodeData, "wiki">;

const HIDDEN_HANDLE = "!pointer-events-none !size-px !min-h-0 !min-w-0 !border-0 !bg-transparent";

const WikiGraphNodeView = memo(function WikiGraphNodeView({ data }: NodeProps<WikiFlowNode>) {
  const { node, degree, selected, dimmed } = data;
  const isSource = node.type === "source";
  const presentation = wikiStatusPresentation(node.status);
  // 关联越多节点越大，帮助识别枢纽条目；限制范围避免遮挡。
  const size = isSource ? 12 : Math.min(28, 14 + degree * 2);
  return (
    <div
      className={cn(
        "flex max-w-40 flex-col items-center gap-1 transition-opacity",
        (dimmed || presentation.faded) && "opacity-40",
      )}
      data-testid="knowledge-graph-node"
      data-node-type={node.type}
      data-node-status={node.status ?? node.sourceStatus ?? ""}
    >
      <Handle
        type="target"
        position={Position.Top}
        isConnectable={false}
        className={HIDDEN_HANDLE}
      />
      <span
        aria-hidden
        className={cn(
          "block border-2",
          isSource
            ? "rounded-sm border-border-hover bg-surface"
            : cn("rounded-full", WIKI_TONE_CLASS[wikiKindTone(node.kind)]),
          !isSource && presentation.border === "solid" && "border-background",
          !isSource && presentation.border === "dashed" && "border-dashed border-foreground-subtle",
          !isSource && presentation.border === "warning" && "border-warning",
          selected && "ring-2 ring-brand ring-offset-2 ring-offset-background",
        )}
        style={{ width: size, height: size }}
      />
      <span
        className={cn(
          "max-w-40 truncate rounded-sm bg-background/80 px-1 text-ui-xs",
          selected ? "font-medium text-foreground" : "text-foreground-subtle",
        )}
        title={node.label}
      >
        {node.label}
      </span>
      <Handle
        type="source"
        position={Position.Bottom}
        isConnectable={false}
        className={HIDDEN_HANDLE}
      />
    </div>
  );
});

const NODE_TYPES = { wiki: WikiGraphNodeView };

export function KnowledgeGraphCanvas({
  nodes,
  edges,
  selectedId,
  onSelect,
  ariaLabel,
}: {
  nodes: readonly V4WikiGraphNode[];
  edges: readonly V4WikiGraphEdge[];
  selectedId: string | null;
  onSelect: (node: V4WikiGraphNode | null) => void;
  ariaLabel: string;
}) {
  // 布局只依赖节点/边集合，选中变化不重新计算位置。
  const positions = useMemo(() => layoutWikiGraph(nodes, edges), [nodes, edges]);
  const degrees = useMemo(() => wikiNodeDegrees(edges), [edges]);
  const neighborhood = useMemo(() => wikiNeighborhood(selectedId, edges), [edges, selectedId]);
  const byId = useMemo(() => new Map(nodes.map((node) => [node.id, node])), [nodes]);

  const flowNodes = useMemo<WikiFlowNode[]>(
    () =>
      nodes.map((node) => ({
        id: node.id,
        type: "wiki",
        position: positions.get(node.id) ?? { x: 0, y: 0 },
        // 位置由确定性布局给出；不接 onNodesChange，因此不可拖动，平移/缩放由画布负责。
        draggable: false,
        connectable: false,
        data: {
          node,
          degree: degrees.get(node.id) ?? 0,
          selected: node.id === selectedId,
          dimmed: Boolean(selectedId) && !neighborhood.has(node.id),
        },
      })),
    [degrees, neighborhood, nodes, positions, selectedId],
  );

  const flowEdges = useMemo<Edge[]>(
    () =>
      edges.map((edge) => {
        const style = wikiEdgeStyle(edge.type);
        const highlighted =
          selectedId !== null && (edge.from === selectedId || edge.to === selectedId);
        return {
          id: edge.id,
          source: edge.from,
          target: edge.to,
          type: "straight",
          selectable: false,
          animated: style.animated,
          data: { type: edge.type },
          style: {
            stroke: style.stroke,
            strokeDasharray: style.strokeDasharray,
            strokeWidth: highlighted ? 2 : 1,
            opacity: selectedId && !highlighted ? 0.25 : 1,
          },
        };
      }),
    [edges, selectedId],
  );

  return (
    <div
      className="h-full w-full"
      role="figure"
      aria-label={ariaLabel}
      data-testid="knowledge-graph-canvas"
    >
      <ReactFlowProvider>
        <ReactFlow
          nodes={flowNodes}
          edges={flowEdges}
          nodeTypes={NODE_TYPES}
          fitView
          minZoom={0.1}
          maxZoom={2.5}
          nodesConnectable={false}
          edgesFocusable={false}
          deleteKeyCode={null}
          proOptions={{ hideAttribution: true }}
          onNodeClick={(_event, flowNode) => onSelect(byId.get(flowNode.id) ?? null)}
          onPaneClick={() => onSelect(null)}
        >
          <Controls showInteractive={false} position="bottom-right" />
        </ReactFlow>
      </ReactFlowProvider>
    </div>
  );
}
