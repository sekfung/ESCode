import type { ChildProcessWithoutNullStreams } from "node:child_process";

import type { RustHostLogBatch } from "./rustHostBridge.js";

/**
 * 控制通道读取（Host→Main 的 NDJSON 帧）。`logBatch` 不是握手消息：交给 `onLogBatch`
 * 消费（缺省丢弃），**不进** `next()` 的队列，否则它们会把握手帧挤到后面
 * （`nextMatching` 有 50 帧上限）、并在测试里被反复跳过。
 */
export function createControlChannel(
  child: ChildProcessWithoutNullStreams,
  handlers: {
    onLogBatch?: (batch: RustHostLogBatch) => void;
    onAuthorizeRequest?: (request: { requestId: string; path: string }) => void;
    onMainRequest?: (request: { requestId: string; method: string; params: unknown }) => void;
    onHostEvent?: (event: unknown) => void;
    onWorkspaceStartupState?: (state: {
      startupId: string;
      windowId: string;
      workspaceKey: string;
      generation: number;
      state: unknown;
    }) => void;
  },
) {
  let buffer = "";
  const pending: Array<(message: Record<string, unknown>) => void> = [];
  const queue: Array<Record<string, unknown>> = [];
  child.stdout.setEncoding("utf8");
  child.stdout.on("data", (chunk: string) => {
    buffer += chunk;
    let newline: number;
    while ((newline = buffer.indexOf("\n")) >= 0) {
      const line = buffer.slice(0, newline);
      buffer = buffer.slice(newline + 1);
      if (!line.trim()) continue;
      const message = JSON.parse(line) as Record<string, unknown>;
      if (message.kind === "logBatch") {
        const batch = toLogBatch(message);
        if (batch) handlers.onLogBatch?.(batch);
        continue;
      }
      // 授权请求同样不进握手队列：它随时可能到达，由 Main 的注册表应答。
      if (message.kind === "localMediaPreviewAuthorize") {
        const { requestId, path } = message;
        if (typeof requestId === "string" && typeof path === "string") {
          handlers.onAuthorizeRequest?.({ requestId, path });
        }
        continue;
      }
      if (message.kind === "mainRequest") {
        const { requestId, method, params } = message;
        if (typeof requestId === "string" && typeof method === "string") {
          handlers.onMainRequest?.({ requestId, method, params });
        }
        continue;
      }
      if (message.kind === "hostEvent") {
        handlers.onHostEvent?.(message.event);
        continue;
      }
      if (message.kind === "workspaceStartupState") {
        const { startupId, windowId, workspaceKey, generation, state } = message;
        if (
          typeof startupId === "string" &&
          typeof windowId === "string" &&
          typeof workspaceKey === "string" &&
          typeof generation === "number"
        ) {
          handlers.onWorkspaceStartupState?.({
            startupId,
            windowId,
            workspaceKey,
            generation,
            state,
          });
        }
        continue;
      }
      const resolveNext = pending.shift();
      if (resolveNext) resolveNext(message);
      else queue.push(message);
    }
  });
  const next = () =>
    new Promise<Record<string, unknown>>((resolveNext) => {
      const queued = queue.shift();
      if (queued) resolveNext(queued);
      else pending.push(resolveNext);
    });
  return {
    next,
    async nextMatching(predicate: (message: Record<string, unknown>) => boolean) {
      for (let i = 0; i < 50; i += 1) {
        const message = await next();
        if (predicate(message)) return message;
      }
      throw new Error("Rust Host control stream did not yield the expected frame");
    },
  };
}

/** `logBatch` 帧的字段校验（协议漂移时退回丢弃，不打断控制通道）。 */
function toLogBatch(message: Record<string, unknown>): RustHostLogBatch | undefined {
  const scope = message.scope;
  const level = message.level;
  const lines = message.lines;
  if (typeof scope !== "string") return undefined;
  if (level !== "debug" && level !== "info" && level !== "warn" && level !== "error") {
    return undefined;
  }
  if (!Array.isArray(lines) || lines.some((line) => typeof line !== "string")) return undefined;
  return { scope, level, lines: lines as string[] };
}
