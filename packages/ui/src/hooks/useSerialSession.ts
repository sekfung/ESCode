/**
 * useSerialSession —— 串口调试器 hook
 *
 * 串口会话的唯一所有者是窗口级 Desktop Local Host 的 SerialService；
 * 这里只镜像其快照与事件，并把用户操作转成 Service 命令。
 */
import { useCallback, useEffect, useState } from "react";
import type { ISerialService, SerialConfig, SerialPortInfo, SerialStatus } from "@zcode/services";
import { logger } from "@/logger.js";
import {
  applySerialChunk,
  applySerialSnapshot,
  clearSerialChunkLog,
  createSerialChunkLog,
  type SerialChunkLog,
} from "@/lib/serial/serialChunkLog.js";

export interface SerialSessionState {
  status: SerialStatus;
  ports: SerialPortInfo[];
  log: SerialChunkLog;
  /** list() 失败（如原生模块不可用）时的错误，与串口状态错误分开展示。 */
  listError: { code?: string; message: string } | null;
}

function toError(error: unknown): { code?: string; message: string } {
  if (error instanceof Error) {
    const code = (error as Error & { code?: unknown }).code;
    return { code: typeof code === "string" ? code : undefined, message: error.message };
  }
  return { message: String(error) };
}

export function useSerialSession(service: ISerialService | undefined, isVisible: boolean) {
  const [state, setState] = useState<SerialSessionState>({
    status: { state: "closed" },
    ports: [],
    log: createSerialChunkLog(),
    listError: null,
  });

  useEffect(() => {
    if (!service) return;
    let cancelled = false;
    setState((current) => ({ ...current, log: createSerialChunkLog() }));
    // 先订阅再取快照：订阅期间先到的 chunk 暂存于 log.pending，快照到达后按 seq 去重合并。
    const subscriptions = [
      service.onData((chunk) => {
        setState((current) => ({ ...current, log: applySerialChunk(current.log, chunk) }));
      }),
      service.onStatus((status) => {
        setState((current) => ({ ...current, status }));
      }),
      service.onPorts((ports) => {
        setState((current) => ({ ...current, ports, listError: null }));
      }),
    ];
    void service
      .getSnapshot()
      .then((snapshot) => {
        if (cancelled) return;
        setState((current) => ({
          ...current,
          status: snapshot.status,
          log: applySerialSnapshot(current.log, snapshot),
        }));
      })
      .catch((error: unknown) => {
        if (!cancelled) logger.warn("[serial] failed to load snapshot", toError(error));
      });
    return () => {
      cancelled = true;
      for (const subscription of subscriptions) subscription.dispose();
    };
  }, [service]);

  const refreshPorts = useCallback(async () => {
    if (!service) return;
    try {
      const ports = await service.list();
      setState((current) => ({ ...current, ports, listError: null }));
    } catch (error) {
      setState((current) => ({ ...current, listError: toError(error) }));
    }
  }, [service]);

  useEffect(() => {
    if (!service) return;
    // 面板可见才让 Host 轮询热插拔；不可见时停止，等待重连的轮询由 Host 自己维持。
    void service.setWatching({ watching: isVisible }).catch((error: unknown) => {
      logger.warn("[serial] failed to update watching", toError(error));
    });
    if (isVisible) void refreshPorts();
  }, [service, isVisible, refreshPorts]);

  useEffect(() => {
    if (!service) return;
    return () => {
      void service.setWatching({ watching: false }).catch(() => {});
    };
  }, [service]);

  const open = useCallback(
    async (path: string, config: SerialConfig) => {
      if (!service) return;
      await service.open({ path, config });
    },
    [service],
  );

  const close = useCallback(async () => {
    if (!service) return;
    await service.close();
  }, [service]);

  const send = useCallback(
    async (bytes: Uint8Array) => {
      if (!service) return;
      await service.write({ bytes, source: "user" });
    },
    [service],
  );

  const clear = useCallback(async () => {
    if (!service) return;
    await service.clear();
    setState((current) => ({ ...current, log: clearSerialChunkLog(current.log) }));
  }, [service]);

  return { state, refreshPorts, open, close, send, clear };
}
