// 知识图谱页的数据访问（apps/escode-cli-rust/docs/llm-wiki/01-spec.md §11「UI 状态所有者与顺序」）。
// 知识事实只属于 runtime：这里保存最近一次响应快照，不做乐观更新；治理操作完成后整体重拉。
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type {
  V4WikiEntryResult,
  V4WikiGraphParams,
  V4WikiGraphResult,
  V4WikiManageResult,
  V4WikiStatusResult,
  WikiKind,
  WikiScope,
  WikiStatus,
} from "@escode/shared/escode-protocol-v4";
import { useWorkspaceServicesResolution } from "@/hooks/useWorkspaceServices.js";
import type { WikiReviewAction } from "@/lib/wikiGraph.js";
import { logger } from "@/logger.js";

export interface WikiKnowledgeTarget {
  workspacePath: string | null | undefined;
  workspaceIdentity?: string;
  remoteSessionId?: string | null;
  remoteTarget?: unknown;
}

export interface WikiGraphFilters {
  scopes: WikiScope[];
  statuses: WikiStatus[];
  kinds: WikiKind[];
  includeSources: boolean;
}

export type WikiKnowledgePhase =
  | "no-workspace"
  | "waiting"
  | "loading"
  | "ready"
  | "disabled"
  | "unsupported"
  | "error";

/** 页面默认请求的节点上限；协议上限为 2000。 */
export const WIKI_GRAPH_DEFAULT_LIMIT = 500;

export const DEFAULT_WIKI_GRAPH_FILTERS: WikiGraphFilters = {
  scopes: [],
  statuses: [],
  kinds: [],
  includeSources: true,
};

/** 空数组表示「不过滤」，不下发该字段（与 runtime 默认一致）。 */
export function buildWikiGraphQuery(filters: WikiGraphFilters): V4WikiGraphParams {
  return {
    ...(filters.scopes.length ? { scopes: filters.scopes } : {}),
    ...(filters.statuses.length ? { statuses: filters.statuses } : {}),
    ...(filters.kinds.length ? { kinds: filters.kinds } : {}),
    includeSources: filters.includeSources,
    limit: WIKI_GRAPH_DEFAULT_LIMIT,
  };
}

/** 状态结果 → 页面阶段；supported=false 来自服务层对 -32601 的折算。 */
export function resolveWikiPhase(status: V4WikiStatusResult): WikiKnowledgePhase {
  if (!status.supported) return "unsupported";
  if (!status.enabled) return "disabled";
  return "ready";
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export function useWikiKnowledge(target: WikiKnowledgeTarget, filters: WikiGraphFilters) {
  const workspacePath = target.workspacePath?.trim() || null;
  const resolution = useWorkspaceServicesResolution(
    workspacePath,
    target.remoteSessionId,
    target.workspaceIdentity,
    target.remoteTarget,
  );
  const agent = resolution.services.escodeAgentService;
  const remoteSessionId = resolution.remoteSessionId ?? undefined;
  const workspaceTarget = useMemo(
    () =>
      workspacePath
        ? { workspacePath, workspaceIdentity: target.workspaceIdentity, remoteSessionId }
        : null,
    [remoteSessionId, target.workspaceIdentity, workspacePath],
  );

  const [phase, setPhase] = useState<WikiKnowledgePhase>(
    workspacePath ? "waiting" : "no-workspace",
  );
  const [status, setStatus] = useState<V4WikiStatusResult | null>(null);
  const [graph, setGraph] = useState<V4WikiGraphResult | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [entry, setEntry] = useState<V4WikiEntryResult | null>(null);
  const [entryLoading, setEntryLoading] = useState(false);
  const [busy, setBusy] = useState(false);
  // 请求序号：只接受最新一次加载的结果；切换 workspace/筛选后旧响应作废。
  const loadSeq = useRef(0);
  const entrySeq = useRef(0);
  const query = useMemo(() => buildWikiGraphQuery(filters), [filters]);

  const load = useCallback(async () => {
    const seq = ++loadSeq.current;
    if (!workspaceTarget) {
      setPhase("no-workspace");
      return;
    }
    if (!resolution.rpcReady) {
      // 远程 workspace 连接未就绪时不发越界 RPC，等待 rpcReady 变化后重新触发。
      setPhase("waiting");
      return;
    }
    if (!agent.getWikiStatus || !agent.getWikiGraph) {
      setPhase("unsupported");
      return;
    }
    setPhase("loading");
    setError(null);
    try {
      const nextStatus = await agent.getWikiStatus(workspaceTarget);
      if (seq !== loadSeq.current) return;
      setStatus(nextStatus);
      const nextPhase = resolveWikiPhase(nextStatus);
      if (nextPhase !== "ready") {
        setGraph(null);
        setPhase(nextPhase);
        return;
      }
      const nextGraph = await agent.getWikiGraph({ ...workspaceTarget, query });
      if (seq !== loadSeq.current) return;
      setGraph(nextGraph);
      setPhase("ready");
    } catch (loadError) {
      if (seq !== loadSeq.current) return;
      logger.warn("[KnowledgeSettings] load failed", { error: errorMessage(loadError) });
      setError(errorMessage(loadError));
      setPhase("error");
    }
  }, [agent, query, resolution.rpcReady, workspaceTarget]);

  const loadEntry = useCallback(
    async (id: string | null) => {
      const seq = ++entrySeq.current;
      setSelectedId(id);
      if (!id || !workspaceTarget || !agent.getWikiEntry) {
        setEntry(null);
        setEntryLoading(false);
        return;
      }
      setEntryLoading(true);
      try {
        const result = await agent.getWikiEntry({ ...workspaceTarget, id });
        if (seq !== entrySeq.current) return;
        setEntry(result);
      } catch (entryError) {
        if (seq !== entrySeq.current) return;
        logger.warn("[KnowledgeSettings] entry load failed", {
          id,
          error: errorMessage(entryError),
        });
        setEntry({ found: false });
      } finally {
        if (seq === entrySeq.current) setEntryLoading(false);
      }
    },
    [agent, workspaceTarget],
  );

  useEffect(() => {
    void load();
  }, [load]);

  // 切换 workspace 时清空选中：不同项目的条目 id 不可互认。
  useEffect(() => {
    entrySeq.current += 1;
    setSelectedId(null);
    setEntry(null);
  }, [workspaceTarget]);

  const review = useCallback(
    async (
      action: WikiReviewAction,
      ids: string[],
      options: { reason?: string; supersededBy?: string } = {},
    ): Promise<V4WikiManageResult | null> => {
      if (!workspaceTarget || !agent.manageWiki || ids.length === 0) return null;
      setBusy(true);
      try {
        const result = await agent.manageWiki({
          ...workspaceTarget,
          request: { action, ids, ...options },
        });
        logger.info("[KnowledgeSettings] review finished", {
          action,
          count: ids.length,
          ok: result.ok,
        });
        return result;
      } finally {
        setBusy(false);
        // 以 runtime 为准：无论成败都重拉，避免界面停留在过期状态。
        await load();
        await loadEntry(selectedId);
      }
    },
    [agent, load, loadEntry, selectedId, workspaceTarget],
  );

  return {
    phase,
    status,
    graph,
    error,
    selectedId,
    entry,
    entryLoading,
    busy,
    canManage: Boolean(agent.manageWiki),
    reload: load,
    select: loadEntry,
    review,
  };
}
