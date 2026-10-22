import { useEffect, useState } from "react";
import type { SvnBranchGraphResult, SvnCredentials, SvnResult } from "@escode/shared";
import { TID_SVN_BRANCH_GRAPH_DIALOG } from "@escode/shared";
import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/components/ui/dialog.js";
import { cn } from "@/components/lib/utils.js";
import { useESCodeIntl } from "@/i18n/IntlProvider.js";
import type { SvnWorkspaceController } from "@/svn/useSvnWorkspace.js";
import { getSvnIssueMessageId } from "@/svn/svnDisplay.js";
import { layoutSvnBranchGraph } from "@/svn/svnGraphLayout.js";

/**
 * 分支拓扑只读视图（spec：docs/svn/ui.md §13）。
 * 实线 = copyfrom；虚线 = mergeinfo；不提供切换/合并操作。
 */
export function SvnBranchGraphDialog({
  open,
  controller,
  runWithCredentialRetry,
  onOpenChange,
}: {
  open: boolean;
  controller: SvnWorkspaceController;
  runWithCredentialRetry: <T extends object>(
    operation: (credentials?: SvnCredentials) => Promise<SvnResult<T>>,
  ) => Promise<SvnResult<T>>;
  onOpenChange: (open: boolean) => void;
}) {
  const { intl } = useESCodeIntl();
  const { svnService, request, state } = controller;
  const [graph, setGraph] = useState<SvnBranchGraphResult | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!open || !svnService) {
      return;
    }
    let cancelled = false;
    setLoading(true);
    setError(null);
    setGraph(null);
    void (async () => {
      try {
        const result = await runWithCredentialRetry((credentials) =>
          svnService.getBranchGraph({
            ...request,
            ...(credentials ? { credentials } : {}),
          }),
        );
        if (cancelled) {
          return;
        }
        if (result.ok) {
          setGraph(result);
        } else if (result.issue !== "cancelled") {
          setError(
            intl.formatMessage(
              { id: getSvnIssueMessageId(result.issue) },
              { message: result.message },
            ),
          );
        }
      } catch (loadError) {
        if (!cancelled) {
          setError(loadError instanceof Error ? loadError.message : String(loadError));
        }
      } finally {
        if (!cancelled) {
          setLoading(false);
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [intl, open, request, runWithCredentialRetry, svnService]);

  const layout = graph ? layoutSvnBranchGraph(graph) : null;
  const currentPath = state.info?.branch?.relativePath;

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        data-testid={TID_SVN_BRANCH_GRAPH_DIALOG}
        className="flex h-[min(78vh,720px)] max-w-5xl flex-col gap-0 overflow-hidden rounded-2xl p-0"
      >
        <DialogHeader className="border-b border-border px-5 py-4">
          <DialogTitle className="text-ui-lg font-medium text-foreground">
            {intl.formatMessage({ id: "svn.graph.title" })}
          </DialogTitle>
          <div className="flex flex-wrap items-center gap-4 text-ui-sm text-foreground-subtle">
            <span className="flex items-center gap-1">
              <svg aria-hidden="true" width="22" height="8" viewBox="0 0 22 8">
                <line x1="1" y1="4" x2="21" y2="4" className="stroke-foreground-subtle" />
              </svg>
              {intl.formatMessage({ id: "svn.graph.legendCopy" })}
            </span>
            <span className="flex items-center gap-1">
              <svg aria-hidden="true" width="22" height="8" viewBox="0 0 22 8">
                <line
                  x1="1"
                  y1="4"
                  x2="21"
                  y2="4"
                  strokeDasharray="5 3"
                  className="stroke-foreground-subtle"
                />
              </svg>
              {intl.formatMessage({ id: "svn.graph.legendMerge" })}
            </span>
          </div>
        </DialogHeader>
        <div className="min-h-0 flex-1 overflow-auto p-4">
          {loading ? (
            <p className="text-ui-base text-foreground-subtle">
              {intl.formatMessage({ id: "common.loading" })}
            </p>
          ) : error ? (
            <p className="whitespace-pre-line text-ui-base text-destructive">{error}</p>
          ) : graph && graph.layout === "non-standard" ? (
            <p className="text-ui-base text-foreground-subtle">
              {intl.formatMessage({ id: "svn.graph.nonStandard" })}
            </p>
          ) : !graph || graph.nodes.length === 0 ? (
            <p className="text-ui-base text-foreground-subtle">
              {intl.formatMessage({ id: "svn.graph.empty" })}
            </p>
          ) : layout ? (
            <>
              <svg
                role="img"
                width={layout.width}
                height={layout.height}
                viewBox={`0 0 ${layout.width} ${layout.height}`}
                className="min-w-full"
              >
                <defs>
                  <marker
                    id="svn-graph-arrow"
                    viewBox="0 0 10 10"
                    refX="9"
                    refY="5"
                    markerWidth="6"
                    markerHeight="6"
                    orient="auto-start-reverse"
                  >
                    <path d="M 0 0 L 10 5 L 0 10 z" className="fill-foreground-subtle" />
                  </marker>
                </defs>
                {layout.edges.map(({ edge, path, labelX, labelY }) => (
                  <g
                    key={`${edge.kind}:${edge.from}:${edge.to}`}
                    data-svn-graph-edge={edge.kind}
                    data-from={edge.from}
                    data-to={edge.to}
                  >
                    <path
                      d={path}
                      fill="none"
                      markerEnd="url(#svn-graph-arrow)"
                      strokeWidth={1.5}
                      strokeDasharray={edge.kind === "merge" ? "6 4" : undefined}
                      className="stroke-foreground-subtle"
                    />
                    <text
                      x={labelX}
                      y={labelY}
                      textAnchor="middle"
                      className="fill-foreground-subtle text-[10px]"
                    >
                      {edge.kind === "copy"
                        ? intl.formatMessage(
                            { id: "svn.graph.copyLabel" },
                            { revision: edge.revision ?? 0 },
                          )
                        : intl.formatMessage(
                            { id: "svn.graph.mergeLabel" },
                            { revisions: edge.revisions ?? "" },
                          )}
                    </text>
                  </g>
                ))}
                {layout.nodes.map(({ node, x, y, width, height }) => {
                  const current = node.relativePath === currentPath;
                  return (
                    <g key={node.relativePath} data-svn-graph-node={node.relativePath}>
                      <rect
                        x={x}
                        y={y}
                        width={width}
                        height={height}
                        rx={10}
                        strokeWidth={current ? 2 : 1}
                        className={cn(
                          "fill-background",
                          current ? "stroke-primary" : "stroke-border",
                        )}
                      />
                      <text x={x + 12} y={y + 18} className="fill-foreground text-[12px]">
                        {node.relativePath || "/"}
                      </text>
                      <text x={x + 12} y={y + 32} className="fill-foreground-subtle text-[10px]">
                        {node.kind}
                        {node.createdRevision !== undefined ? ` · r${node.createdRevision}` : ""}
                      </text>
                    </g>
                  );
                })}
              </svg>
              {graph.truncated ? (
                <p className="mt-3 text-ui-sm text-foreground-subtle">
                  {intl.formatMessage({ id: "svn.graph.truncated" }, { count: graph.nodes.length })}
                </p>
              ) : null}
            </>
          ) : null}
        </div>
      </DialogContent>
    </Dialog>
  );
}
