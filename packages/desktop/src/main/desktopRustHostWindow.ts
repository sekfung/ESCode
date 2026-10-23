/**
 * 窗口 → App 级 Rust Host 的 attachment（H2.3，docs/specs/rust-u2-window-host.md）。
 *
 * 与 Node utility Host 的 `MessageChannelMain` 形状相同：renderer 仍把端口当作
 * `InternalChannels.ServicePort` 消费；Main 负责把另一端口桥到 Rust Host。
 * 窗口关闭/reload 只解绑 attachment，不停 Host。
 *
 * 启动门使用实例真实的 `startup/storageState`（`WorkspacePrepare`），不再由 Main 合成 ready。
 */

import { randomUUID } from "node:crypto";
import { ipcMain, MessageChannelMain, type BrowserWindow } from "electron";
import {
  databaseStartupControlSchema,
  InternalChannels,
  type DatabaseStartupState,
} from "@escode/shared";
import { logDatabaseStartupTerminal, requestDatabaseStartupQuit } from "./databaseStartupRelay.js";
import type { RustHostManager } from "./rustHostManager.js";

export interface RustWindowAttachment {
  readonly startupId: string;
  dispose(): void;
}

interface ActiveRustWindow {
  win: BrowserWindow;
  manager: RustHostManager;
  logger: { info: (...args: unknown[]) => void; warn: (...args: unknown[]) => void };
  startupId: string;
  windowId: string;
  workspace?: RustWorkspaceTarget;
  runtimeEnvPatch?: Record<string, string>;
  attachment: Awaited<ReturnType<RustHostManager["attachLocalPort"]>>;
  windowAttachment?: RustWindowAttachment;
  sendState(state: DatabaseStartupState): void;
  nextSequence(): number;
}

const activeWindows = new Map<number, ActiveRustWindow>();

/** 解绑一个窗口；只关 attachment，不停 Host。 */
export function disposeRustHostWindow(windowId: number): void {
  const active = activeWindows.get(windowId);
  if (!active) return;
  activeWindows.delete(windowId);
  if (active.windowAttachment) active.windowAttachment.dispose();
  else active.attachment.dispose();
}

/** Host 重启中：窗口保持挂载，启动门回到 checking（不伪造 failed）。 */
export function markRustHostWindowsRestarting(): void {
  for (const active of activeWindows.values()) {
    active.sendState({
      schemaVersion: 1,
      startupId: active.startupId,
      attemptId: randomUUID(),
      sequence: active.nextSequence(),
      startedAt: Date.now(),
      updatedAt: Date.now(),
      phase: "preparing_session_storage",
      databasePhase: "checking",
      disk: [],
    });
  }
}

/** Host 放弃重启：所有窗口进入可重试的 failed。 */
export function failRustHostWindows(code: DatabaseStartupState["errorCode"]): void {
  for (const active of activeWindows.values()) {
    active.sendState({
      schemaVersion: 1,
      startupId: active.startupId,
      attemptId: randomUUID(),
      sequence: active.nextSequence(),
      startedAt: Date.now(),
      updatedAt: Date.now(),
      phase: "failed",
      failedPhase: "preparing_session_storage",
      errorCode: code,
      disk: [],
    });
  }
}

/** Host 恢复：为所有窗口重新 attach（新 startupId + 新端口；不在旧端口原地重连）。 */
export async function reattachRustHostWindows(): Promise<void> {
  for (const windowId of Array.from(activeWindows.keys())) {
    const active = activeWindows.get(windowId);
    if (!active) continue;
    if (active.win.isDestroyed()) {
      activeWindows.delete(windowId);
      continue;
    }
    try {
      disposeRustHostWindow(windowId);
      await attachRustHostWindow({
        win: active.win,
        manager: active.manager,
        logger: active.logger,
        windowId: active.windowId,
        workspace: active.workspace,
        runtimeEnvPatch: active.runtimeEnvPatch,
        // 旧 renderer 已挂载业务根；先发 state 让其按恢复流程 reload，新页面再走 snapshot。
        rendererListening: true,
      });
    } catch (error) {
      active.logger.warn(`[createWindow] Rust Host reattach failed (${active.windowId}):`, error);
    }
  }
}

export interface RustWorkspaceTarget {
  path: string;
  identity?: string;
  dataDir?: string;
  config?: string;
}

const STARTUP_TIMEOUT_MS = 20_000;

const VALID_DATABASE_PHASES = new Set([
  "checking",
  "waiting_for_lock",
  "migrating",
  "committing",
  "maintaining",
  "ready",
]);
const VALID_ERROR_CODES = new Set([
  "storage_full",
  "permission_denied",
  "io_error",
  "out_of_memory",
  "corrupt",
  "open_failed",
  "lock_timeout",
  "checksum_mismatch",
  "sql_failed",
  "startup_status_timeout",
  "transport_closed",
  "unsupported_runtime",
]);

function databasePhase(value: unknown): DatabaseStartupState["databasePhase"] {
  return typeof value === "string" && VALID_DATABASE_PHASES.has(value)
    ? (value as DatabaseStartupState["databasePhase"])
    : undefined;
}

function errorCode(value: unknown): DatabaseStartupState["errorCode"] {
  return typeof value === "string" && VALID_ERROR_CODES.has(value)
    ? (value as DatabaseStartupState["errorCode"])
    : "sql_failed";
}

/** Host 的 `startup/storageState` → renderer 的 `DatabaseStartupState`（严格 schema）。 */
export function toDatabaseStartupState(
  hostState: unknown,
  startupId: string,
  attemptId: string,
): DatabaseStartupState {
  const record =
    hostState && typeof hostState === "object" ? (hostState as Record<string, unknown>) : {};
  const sequence =
    typeof record.sequence === "number" && Number.isFinite(record.sequence)
      ? Math.max(0, Math.trunc(record.sequence))
      : 1;
  const startedAt =
    typeof record.startedAt === "number" && Number.isFinite(record.startedAt)
      ? record.startedAt
      : Date.now();
  const phase = record.phase;
  const common = {
    schemaVersion: 1 as const,
    startupId,
    attemptId,
    sequence,
    startedAt,
    updatedAt: Date.now(),
    disk: [],
  };
  if (phase === "ready") {
    return { ...common, phase: "ready", databasePhase: "ready" };
  }
  if (phase === "failed") {
    return {
      ...common,
      phase: "failed",
      failedPhase: "preparing_session_storage",
      errorCode: errorCode(record.errorCode),
    };
  }
  const mapped = databasePhase(phase);
  return {
    ...common,
    phase: "preparing_session_storage",
    ...(mapped ? { databasePhase: mapped } : {}),
  };
}

function terminalState(
  startupId: string,
  attemptId: string,
  sequence: number,
  phase: "ready" | "failed",
  startedAt: number,
  code?: DatabaseStartupState["errorCode"],
): DatabaseStartupState {
  return {
    schemaVersion: 1,
    startupId,
    attemptId,
    sequence,
    startedAt,
    updatedAt: Date.now(),
    phase,
    ...(phase === "ready" ? { databasePhase: "ready" as const } : {}),
    ...(phase === "failed"
      ? { failedPhase: "preparing_session_storage" as const, errorCode: code ?? "transport_closed" }
      : {}),
    disk: [],
  };
}

/**
 * 绑定一个窗口的 Rust Host attachment。
 *
 * 提供 `workspace` 时先发 `WorkspacePrepare` 并等待实例真实 ready；Host 的中间态会
 * 实时转发给 renderer。未提供时保留旧的合成 ready（仅测试/无 workspace 场景）。
 */
export async function attachRustHostWindow(options: {
  win: BrowserWindow;
  manager: RustHostManager;
  logger: { info: (...args: unknown[]) => void; warn: (...args: unknown[]) => void };
  startupId?: string;
  windowId?: string;
  workspace?: RustWorkspaceTarget;
  runtimeEnvPatch?: Record<string, string>;
  /** 窗口已挂载业务根（Host 重启后的 reattach）：无需等待新的 renderer snapshot。 */
  rendererListening?: boolean;
  onLost?: () => void;
}): Promise<RustWindowAttachment> {
  const { win, manager, logger } = options;
  const startupId = options.startupId ?? randomUUID();
  const attemptId = randomUUID();
  const startedAt = Date.now();
  let disposed = false;
  let state: DatabaseStartupState | undefined;
  let latestHostState: unknown;
  let resolveTerminal: (() => void) | undefined;
  // Rust Host 的 ready 可能早于 renderer 模块注册 message 监听（见 renderer
  // requestStartupSnapshot）。renderer 通过 DatabaseStartupControl(snapshot) 明确宣告
  // 监听已就绪前，Main 不能投递 state/ServicePort，否则消息丢失后只能靠 reload 恢复。
  let rendererReady = options.rendererListening === true;
  let portDelivered = false;
  let attachment: Awaited<ReturnType<RustHostManager["attachLocalPort"]>> | undefined;

  const sendStateToRenderer = (next: DatabaseStartupState) => {
    if (!rendererReady || disposed || win.isDestroyed() || win.webContents.isDestroyed()) return;
    win.webContents.send(InternalChannels.DatabaseStartupState, next);
  };

  const deliverPort = () => {
    if (!rendererReady || portDelivered || disposed || !attachment) return;
    if (state?.phase !== "ready") return;
    if (win.isDestroyed() || win.webContents.isDestroyed()) return;
    portDelivered = true;
    win.webContents.postMessage(InternalChannels.ServicePort, { databaseStartupId: startupId }, [
      attachment.port as Electron.MessagePortMain,
    ]);
    sendStateToRenderer(state);
    logger.info(`[createWindow] attached renderer to Rust Host (startupId=${startupId})`);
  };

  const forward = (next: DatabaseStartupState) => {
    if (disposed || win.isDestroyed() || win.webContents.isDestroyed()) return;
    if (state && next.sequence <= state.sequence && next.startupId === state.startupId) return;
    state = next;
    sendStateToRenderer(next);
    if (next.phase === "ready") deliverPort();
    if (next.phase === "ready" || next.phase === "failed") {
      logDatabaseStartupTerminal(next);
      resolveTerminal?.();
    }
  };

  const unsubscribeStartup = options.workspace
    ? manager.onWorkspaceStartupState((event) => {
        if (disposed || event.startupId !== startupId) return;
        latestHostState = event.state;
        forward(toDatabaseStartupState(event.state, startupId, attemptId));
      })
    : undefined;

  const control = (event: Electron.IpcMainEvent, raw: unknown) => {
    if (disposed || event.sender !== win.webContents) return;
    const parsed = databaseStartupControlSchema.safeParse(raw);
    if (!parsed.success) return;
    if (parsed.data.action === "exit") {
      requestDatabaseStartupQuit();
      return;
    }
    if (parsed.data.action === "snapshot") {
      // snapshot 由 renderer 在注册 message 监听之后发送，代表可以安全投递 state/port；
      // 首个 snapshot 可能早于 attachLocalPort 完成，attach 完成后 deliverPort 会再次尝试。
      rendererReady = true;
      if (state) sendStateToRenderer(state);
      deliverPort();
      return;
    }
    if (parsed.data.action === "retry" && options.workspace) {
      manager.resetRestartBudget();
      manager.sendControl({
        kind: "workspaceStartupRetry",
        startupId,
        windowId: options.windowId ?? String(win.webContents.id),
        workspace: options.workspace,
      });
      void reattachRustHostWindows();
    }
  };

  try {
    attachment = await manager.attachLocalPort(
      () => new MessageChannelMain(),
      "desktop-continuous",
      options.workspace
        ? {
            startupId,
            windowId: options.windowId ?? String(win.webContents.id),
            workspace: options.workspace,
            ...(options.runtimeEnvPatch ? { runtimeEnvPatch: options.runtimeEnvPatch } : {}),
          }
        : undefined,
    );
  } catch (error) {
    // 启动失败不静默挂起：renderer 收到 failed 后看到 Main 的显式错误。
    forward(terminalState(startupId, attemptId, 1, "failed", startedAt));
    unsubscribeStartup?.();
    throw error;
  }

  const hostAttachment = attachment!;
  const dispose = () => {
    if (disposed) return;
    disposed = true;
    unsubscribeStartup?.();
    ipcMain.removeListener(InternalChannels.DatabaseStartupControl, control);
    win.removeListener("closed", dispose);
    if (activeWindows.get(win.webContents.id)?.windowAttachment?.dispose === dispose) {
      activeWindows.delete(win.webContents.id);
    }
    hostAttachment.dispose();
  };

  ipcMain.on(InternalChannels.DatabaseStartupControl, control);
  win.once("closed", dispose);
  hostAttachment.onLost(() => {
    if (disposed) return;
    // Host 可能被 manager 自动重启：先回到 checking；manager failed 时再发终态。
    forward({
      schemaVersion: 1,
      startupId,
      attemptId,
      sequence: (state?.sequence ?? 1) + 1,
      startedAt,
      updatedAt: Date.now(),
      phase: "preparing_session_storage",
      databasePhase: "checking",
      disk: [],
    });
    options.onLost?.();
  });

  const windowAttachment: RustWindowAttachment = { startupId, dispose };
  if (options.workspace) {
    activeWindows.set(win.webContents.id, {
      win,
      manager,
      logger,
      startupId,
      windowId: options.windowId ?? String(win.webContents.id),
      workspace: options.workspace,
      ...(options.runtimeEnvPatch ? { runtimeEnvPatch: options.runtimeEnvPatch } : {}),
      attachment: hostAttachment,
      windowAttachment,
      sendState: forward,
      nextSequence: () => (state?.sequence ?? 1) + 1,
    });
  }

  if (options.workspace) {
    if (latestHostState) {
      forward(toDatabaseStartupState(latestHostState, startupId, attemptId));
    }
    if (state?.phase !== "ready" && state?.phase !== "failed") {
      const terminal = new Promise<void>((resolve) => {
        resolveTerminal = resolve;
      });
      const timeout = new Promise<void>((resolve) => {
        setTimeout(resolve, STARTUP_TIMEOUT_MS);
      });
      await Promise.race([terminal, timeout]);
    }
    if (state?.phase !== "ready") {
      if (state?.phase !== "failed") {
        forward(
          terminalState(
            startupId,
            attemptId,
            (state?.sequence ?? 1) + 1,
            "failed",
            startedAt,
            "startup_status_timeout",
          ),
        );
      }
      logger.warn(
        `[createWindow] Rust Host workspace startup did not become ready (startupId=${startupId})`,
      );
      return windowAttachment;
    }
  } else {
    const ready = terminalState(startupId, attemptId, 1, "ready", startedAt);
    forward(ready);
  }

  // renderer 可能尚未发送 snapshot；此时只保留 attachment，待 snapshot 到达后再投递端口。
  deliverPort();

  return windowAttachment;
}
