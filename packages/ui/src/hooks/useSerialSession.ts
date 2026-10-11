/**
 * useSerialSession —— 串口调试器 hook（一个面板标签对应一个串口路径）
 *
 * 串口会话的唯一所有者是窗口级 Desktop Local Host 的 SerialService；
 * 这里只镜像绑定 path 的快照与事件，并把用户操作转成 Service 命令。
 */
import { useCallback, useEffect, useState } from "react";
import type {
  ISerialService,
  SerialConfig,
  SerialPortInfo,
  SerialSessionSummary,
  SerialSignalPulse,
  SerialStatus,
} from "@escode/services";
import { logger } from "@/logger.js";
import {
  applySerialChunk,
  applySerialSnapshot,
  clearSerialChunkLog,
  createSerialChunkLog,
  type SerialChunkLog,
} from "@/lib/serial/serialChunkLog.js";
import { acquireSerialWatch } from "@/hooks/serialWatch.js";

export interface SerialSessionState {
  status: SerialStatus;
  ports: SerialPortInfo[];
  /** 窗口内活动的串口会话（含其他标签打开的），用于“运行中”标注与挂回。 */
  sessions: SerialSessionSummary[];
  log: SerialChunkLog;
  /** 串口列表与会话列表都已加载过一次；预选串口必须等两者齐备。 */
  inventoryLoaded: boolean;
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

const CLOSED: SerialStatus = { state: "closed" };

export function useSerialSession(
  service: ISerialService | undefined,
  path: string | undefined,
  isVisible: boolean,
) {
  const [state, setState] = useState<SerialSessionState>({
    status: CLOSED,
    ports: [],
    sessions: [],
    log: createSerialChunkLog(),
    inventoryLoaded: false,
    listError: null,
  });

  const refreshSessions = useCallback(async () => {
    if (!service) return;
    try {
      const sessions = await service.listSessions();
      setState((current) => ({ ...current, sessions }));
    } catch (error) {
      logger.warn("[serial] failed to list sessions", toError(error));
    }
  }, [service]);

  // 绑定的 path 变化（选择或挂回其他串口）时重建镜像：先订阅再取快照，按 seq 去重合并。
  useEffect(() => {
    if (!service) return;
    let cancelled = false;
    setState((current) => ({
      ...current,
      status: path ? { state: "closed", path } : CLOSED,
      log: createSerialChunkLog(),
    }));
    const subscriptions = [
      service.onData((chunk) => {
        if (chunk.path !== path) return;
        setState((current) => ({ ...current, log: applySerialChunk(current.log, chunk) }));
      }),
      service.onStatus((status) => {
        // 任何会话的状态变化都可能改变“运行中”列表。
        void refreshSessions();
        if (status.path !== path) return;
        setState((current) => ({ ...current, status }));
      }),
      service.onPorts((ports) => {
        setState((current) => ({ ...current, ports, listError: null }));
      }),
    ];
    if (path) {
      void service
        .getSnapshot({ path })
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
    }
    return () => {
      cancelled = true;
      for (const subscription of subscriptions) subscription.dispose();
    };
  }, [path, refreshSessions, service]);

  const refreshPorts = useCallback(async () => {
    if (!service) return;
    // 修复：之前先写入串口列表、再单独取会话列表，新标签的预选会在两者之间执行，
    // 把已被其他标签打开的串口当成空闲而挂回它。现在两者一起取、一次写入。
    const [ports, sessions] = await Promise.allSettled([service.list(), service.listSessions()]);
    setState((current) => ({
      ...current,
      ...(ports.status === "fulfilled" ? { ports: ports.value, listError: null } : {}),
      ...(ports.status === "rejected" ? { listError: toError(ports.reason) } : {}),
      ...(sessions.status === "fulfilled" ? { sessions: sessions.value } : {}),
      inventoryLoaded: true,
    }));
  }, [service]);

  useEffect(() => {
    if (!service || !isVisible) return;
    // 面板可见才让 Host 轮询热插拔（多面板按引用计数）；等待重连的轮询由 Host 自己维持。
    const release = acquireSerialWatch(service);
    void refreshPorts();
    return release;
  }, [service, isVisible, refreshPorts]);

  const open = useCallback(
    async (config: SerialConfig) => {
      if (!service || !path) return;
      await service.open({ path, config });
    },
    [path, service],
  );

  const close = useCallback(async () => {
    if (!service || !path) return;
    await service.close({ path });
  }, [path, service]);

  const send = useCallback(
    async (bytes: Uint8Array) => {
      if (!service || !path) return;
      await service.write({ path, bytes, source: "user" });
    },
    [path, service],
  );

  const setSignals = useCallback(
    async (params: { dtr?: boolean; rts?: boolean; pulse?: SerialSignalPulse }) => {
      if (!service || !path) return;
      await service.setSignals({ path, ...params });
    },
    [path, service],
  );

  const startLoop = useCallback(
    async (params: { bytes: Uint8Array; intervalMs: number; count?: number }) => {
      if (!service || !path) return;
      await service.startLoop({ path, ...params });
    },
    [path, service],
  );

  const stopLoop = useCallback(async () => {
    if (!service || !path) return;
    await service.stopLoop({ path });
  }, [path, service]);

  const clear = useCallback(async () => {
    if (!service || !path) return;
    await service.clear({ path });
    setState((current) => ({ ...current, log: clearSerialChunkLog(current.log) }));
  }, [path, service]);

  return { state, refreshPorts, open, close, send, clear, setSignals, startLoop, stopLoop };
}
