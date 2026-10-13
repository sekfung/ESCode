import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { CircuitCorrectionListEntry, CircuitLayout, ICircuitService } from "@escode/services";
import type {
  CircuitComponent,
  CircuitCorrectionOp,
  CircuitDiagnostic,
  CircuitIrSummary,
  CircuitNet,
} from "@escode/shared/circuit";
import { toCircuitIrSummary } from "@escode/shared/circuit";
import { logger } from "@/logger.js";
import {
  circuitPanelFileKind,
  describeCircuitError,
  isStaleRevisionError,
  parseCircuitSnapshot,
  shouldRefreshCircuitPanel,
  type CircuitPanelError,
  type CircuitPanelFileKind,
} from "@/circuit/domain/circuitPanelSource.js";

/**
 * useCircuitPanel —— 电路面板的数据面（规格 §3.3、§3.4、§8）。
 *
 * IR 的唯一所有者是窗口 Host 里的 `CircuitService`：这里只做读取、订阅与命令转发，
 * 不复制 IR 事实、不自己在 renderer 改数据。刷新规则直接照抄契约：
 * - 事件 `revision` 大于本地才刷新；`revision` 不变但 `stale` 翻转也要刷新（§3.4）。
 * - 写入一律带 `baseRevision`；被 `stale_revision` 拒绝时只提示刷新，不覆盖本地数据。
 * 导出快照（`.circuit.json`）是只读派生产物，只在 renderer 解析展示，绝不回写服务。
 */

const MAX_QUERY_PAGES = 20;

export type CircuitPanelStatus =
  | "unavailable"
  | "unsupported"
  | "loading"
  | "ready"
  | "empty"
  | "error";

export interface CircuitPanelData {
  irId: string;
  revision: number;
  stale: boolean;
  summary: CircuitIrSummary;
  diagnostics: CircuitDiagnostic[];
  components: CircuitComponent[];
  nets: CircuitNet[];
  layout?: CircuitLayout;
  corrections: CircuitCorrectionListEntry[];
  /** 元素列表超过分页上限被截断（BOM/网络表可能不完全）。 */
  truncated: boolean;
}

export interface CircuitPanelState {
  status: CircuitPanelStatus;
  fileKind: CircuitPanelFileKind;
  data: CircuitPanelData | null;
  /** baseRevision 过期被拒：面板提示刷新，数据保持原样。 */
  writeConflict: boolean;
  /** `.circuit.json` 导出快照：只读展示，修正入口不可用。 */
  readOnly: boolean;
  error: CircuitPanelError | null;
}

export interface CircuitWorkspaceScope {
  workspacePath: string;
  workspaceIdentity?: string;
  remoteSessionId?: string;
}

export interface UseCircuitPanelOptions {
  service: ICircuitService | undefined;
  workspace: CircuitWorkspaceScope | null;
  /** workspace 相对路径（服务契约要求，调用方负责把绝对路径转相对）。 */
  path: string | undefined;
  /**
   * `::circuit` 卡片带来的 irId（§8）：优先按它读取已存在的 IR。
   * 卡片只引用已存在的 IR；找不到该 irId 时按 not_found 报错，由用户重新打开。
   */
  requestedIrId?: string;
  /** 导出快照文本；PreviewPane 已经读过的文件内容复用给它，避免二次 IO。 */
  snapshotText?: string | null;
  enabled: boolean;
}

const IDLE_STATE: CircuitPanelState = {
  status: "loading",
  fileKind: "unsupported",
  data: null,
  writeConflict: false,
  readOnly: false,
  error: null,
};

function toWorkspaceRef(workspace: CircuitWorkspaceScope) {
  return {
    workspacePath: workspace.workspacePath,
    ...(workspace.workspaceIdentity ? { workspaceIdentity: workspace.workspaceIdentity } : {}),
    ...(workspace.remoteSessionId ? { remoteSessionId: workspace.remoteSessionId } : {}),
  };
}

async function collectPaged<T>(
  fetchPage: (cursor?: string) => Promise<{ items: T[]; nextCursor?: string }>,
): Promise<{ items: T[]; truncated: boolean }> {
  const items: T[] = [];
  let cursor: string | undefined;
  for (let page = 0; page < MAX_QUERY_PAGES; page += 1) {
    const result = await fetchPage(cursor);
    items.push(...result.items);
    if (!result.nextCursor) return { items, truncated: false };
    cursor = result.nextCursor;
  }
  return { items, truncated: true };
}

export function useCircuitPanel(options: UseCircuitPanelOptions) {
  const { service, workspace, path, requestedIrId, snapshotText, enabled } = options;
  const [state, setState] = useState<CircuitPanelState>(IDLE_STATE);
  /** 「还没有 IR / 读取失败」时的重试开关：重跑一次解析流程，不改变写入路径。 */
  const [resolveNonce, setResolveNonce] = useState(0);
  // 本地已知 revision：事件比对与 correct 都读它，避免闭包读到旧 state。
  const revisionRef = useRef<{ irId: string; revision: number; stale: boolean } | null>(null);
  const workspaceRef = useMemo(() => toWorkspaceRef(workspace ?? { workspacePath: "" }), [workspace]);
  const fileKind = useMemo(() => circuitPanelFileKind(path), [path]);

  const applyData = useCallback((data: CircuitPanelData) => {
    revisionRef.current = { irId: data.irId, revision: data.revision, stale: data.stale };
    setState((current) => ({
      ...current,
      status: "ready",
      data,
      error: null,
    }));
  }, []);

  const load = useCallback(
    async (irId: string, source: "initial" | "refresh"): Promise<void> => {
      if (!service) return;
      const view = await service.get({ ...workspaceRef, irId });
      const [layout, components, nets, corrections] = await Promise.all([
        service.getLayout({ ...workspaceRef, irId }),
        collectPaged((cursor) =>
          service
            .query({ ...workspaceRef, irId, by: "ref", ...(cursor ? { cursor } : {}) })
            .then((result) => (result.by === "ref" ? result : { items: [] })),
        ),
        collectPaged((cursor) =>
          service
            .query({ ...workspaceRef, irId, by: "net", ...(cursor ? { cursor } : {}) })
            .then((result) => (result.by === "net" ? result : { items: [] })),
        ),
        service.listCorrections({ ...workspaceRef, irId }),
      ]);
      if (source === "refresh")
        logger.debug("[circuit] panel refreshed", {
          irId,
          revision: view.summary.revision,
          stale: view.summary.stale,
        });
      applyData({
        irId,
        revision: view.summary.revision,
        stale: view.summary.stale,
        summary: view.summary,
        diagnostics: view.diagnostics,
        components: components.items,
        nets: nets.items,
        ...(layout ? { layout } : {}),
        corrections,
        truncated: components.truncated || nets.truncated,
      });
    },
    [applyData, service, workspaceRef],
  );

  // 初次解析 / 读取（§8「还没有 IR 时」）：卡片带来的 irId 直接读取已存在的 IR；
  // 没有 requestedIrId 时先按源文件找回已有的 IR；KiCad 文件没有 IR 时请求确定性解析。
  useEffect(() => {
    if (!enabled) return;
    let cancelled = false;
    if (!service) {
      setState({ ...IDLE_STATE, status: "unavailable", fileKind });
      return () => {
        cancelled = true;
      };
    }
    if (!workspace || !path || fileKind === "unsupported") {
      setState({ ...IDLE_STATE, status: "unsupported", fileKind });
      return () => {
        cancelled = true;
      };
    }
    setState((current) => ({ ...current, status: "loading", fileKind, error: null }));
    const run = async () => {
      if (fileKind === "snapshot") {
        // 导出快照：派生产物，只读展示，不解析、不订阅（§2 规则 8）。
        const parsed = parseCircuitSnapshot(snapshotText);
        if (cancelled) return;
        if (!parsed.ok) {
          setState({ ...IDLE_STATE, status: "error", fileKind, readOnly: true, error: { message: parsed.reason } });
          return;
        }
        revisionRef.current = {
          irId: parsed.ir.irId,
          revision: parsed.ir.revision,
          stale: parsed.ir.stale,
        };
        setState((current) => ({
          ...current,
          status: "ready",
          fileKind,
          readOnly: true,
          error: null,
          data: {
            irId: parsed.ir.irId,
            revision: parsed.ir.revision,
            stale: parsed.ir.stale,
            // 与服务的 get() 保持同一口径：摘要直接用 shared 的换算，不在这里另算一份。
            summary: toCircuitIrSummary(parsed.ir),
            diagnostics: parsed.ir.diagnostics,
            components: parsed.ir.components,
            nets: parsed.ir.nets,
            corrections: [],
            truncated: false,
          },
        }));
        return;
      }
      try {
        const existing = requestedIrId
          ? { irId: requestedIrId }
          : await service.findBySource({ ...workspaceRef, path });
        if (existing) {
          if (cancelled) return;
          await load(existing.irId, "initial");
          return;
        }
        const parsed = await service.parse({ ...workspaceRef, path });
        if (cancelled) return;
        await load(parsed.summary.irId, "initial");
      } catch (error) {
        if (cancelled) return;
        setState({ ...IDLE_STATE, status: "empty", fileKind, error: describeCircuitError(error) });
      }
    };
    void run();
    return () => {
      cancelled = true;
    };
  }, [enabled, fileKind, load, path, requestedIrId, resolveNonce, service, snapshotText, workspace, workspaceRef]);

  // 事件订阅：同一 irId 才处理；revision 前进或 stale 翻转都触发刷新（§3.3、§3.4）。
  useEffect(() => {
    if (!enabled || !service || fileKind === "snapshot") return;
    const subscription = service.onChanged((event) => {
      const known = revisionRef.current;
      if (!shouldRefreshCircuitPanel(known, event)) return;
      void load(event.irId, event.revision > known!.revision ? "refresh" : "initial").catch(
        (error: unknown) => logger.warn("[circuit] panel refresh failed", describeCircuitError(error)),
      );
    });
    return () => subscription.dispose();
  }, [enabled, fileKind, load, service]);

  const refresh = useCallback(async () => {
    const known = revisionRef.current;
    if (!known || !service) return;
    await load(known.irId, "refresh");
  }, [load, service]);

  /** 重新解析（stale 提示上的操作）：带当前 baseRevision 请求重解析，修正按元素 id 重新套用。 */
  const reparse = useCallback(async () => {
    const known = revisionRef.current;
    if (!service || !workspace || !path || !known) return;
    try {
      const view = await service.parse({
        ...workspaceRef,
        path,
        baseRevision: known.revision,
      });
      await load(view.summary.irId, "refresh");
      setState((current) => ({ ...current, writeConflict: false }));
    } catch (error) {
      setState((current) => ({
        ...current,
        writeConflict: isStaleRevisionError(error),
        error: describeCircuitError(error),
      }));
    }
  }, [load, path, service, workspace, workspaceRef]);

  /** 修正：写失败（stale_revision）时不改本地数据，只置 writeConflict 让面板提示刷新。 */
  const correct = useCallback(
    async (ops: readonly CircuitCorrectionOp[]): Promise<boolean> => {
      const known = revisionRef.current;
      if (!service || !known || ops.length === 0) return false;
      try {
        await service.correct({
          ...workspaceRef,
          irId: known.irId,
          baseRevision: known.revision,
          ops: [...ops],
          author: { kind: "user" },
        });
        await load(known.irId, "refresh");
        setState((current) => ({ ...current, writeConflict: false, error: null }));
        return true;
      } catch (error) {
        setState((current) => ({
          ...current,
          writeConflict: isStaleRevisionError(error),
          error: describeCircuitError(error),
        }));
        return false;
      }
    },
    [load, service, workspaceRef],
  );

  /** 删除失效（orphaned）修正：只有失效记录能删，服务端会再校验一次。 */
  const dropCorrection = useCallback(
    async (index: number): Promise<void> => {
      const known = revisionRef.current;
      if (!service || !known) return;
      try {
        await service.dropCorrection({
          ...workspaceRef,
          irId: known.irId,
          baseRevision: known.revision,
          index,
        });
        await load(known.irId, "refresh");
        setState((current) => ({ ...current, writeConflict: false, error: null }));
      } catch (error) {
        setState((current) => ({
          ...current,
          writeConflict: isStaleRevisionError(error),
          error: describeCircuitError(error),
        }));
      }
    },
    [load, service, workspaceRef],
  );

  const dismissWriteConflict = useCallback(
    () => setState((current) => ({ ...current, writeConflict: false })),
    [],
  );

  /** 还没有 IR / 读取失败时的重试：重跑解析流程（不改写路径，也不假装已经有 IR）。 */
  const retry = useCallback(() => setResolveNonce((nonce) => nonce + 1), []);

  return {
    state,
    fileKind,
    refresh,
    reparse,
    retry,
    correct,
    dropCorrection,
    dismissWriteConflict,
  };
}
