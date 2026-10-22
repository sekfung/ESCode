/**
 * 窗口 → App 级 Rust Host 的 attachment（H2.3，docs/specs/rust-u2-window-host.md）。
 *
 * 与 Node utility Host 的 `MessageChannelMain` 形状相同：renderer 仍把端口当作
 * `InternalChannels.ServicePort` 消费；Main 负责把另一端口桥到 Rust Host。
 * 窗口关闭/reload 只解绑 attachment，不停 Host。
 */

import { randomUUID } from "node:crypto";
import { ipcMain, MessageChannelMain, type BrowserWindow } from "electron";
import {
  databaseStartupControlSchema,
  InternalChannels,
  type DatabaseStartupState,
} from "@escode/shared";
import {
  logDatabaseStartupTerminal,
  requestDatabaseStartupQuit,
} from "./databaseStartupRelay.js";
import type { RustHostManager } from "./rustHostManager.js";

export interface RustWindowAttachment {
  readonly startupId: string;
  dispose(): void;
}

function startupState(
  startupId: string,
  attemptId: string,
  sequence: number,
  phase: "ready" | "failed",
  startedAt: number,
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
      ? { failedPhase: "ready" as const, errorCode: "transport_closed" as const }
      : {}),
    disk: [],
  };
}

/**
 * 绑定一个窗口的 Rust Host attachment。`ready` 由 Main 合成：Rust Host 的实例级
 * `startup/storageState` 不经过 Node Host 的 database-startup relay；在 H2.3 的
 * 真实协议落地前，启动门只用于放行端口。
 */
export async function attachRustHostWindow(options: {
  win: BrowserWindow;
  manager: RustHostManager;
  logger: { info: (...args: unknown[]) => void; warn: (...args: unknown[]) => void };
  onLost?: () => void;
}): Promise<RustWindowAttachment> {
  const { win, manager, logger } = options;
  const startupId = randomUUID();
  const attemptId = randomUUID();
  const startedAt = Date.now();
  let attachment: Awaited<ReturnType<RustHostManager["attachLocalPort"]>>;
  try {
    attachment = await manager.attachLocalPort(
      () => new MessageChannelMain(),
      "desktop-continuous",
    );
  } catch (error) {
    // 启动失败不静默挂起：renderer 先收到 failed，再看到 Main 的显式错误。
    const failed = startupState(startupId, attemptId, 1, "failed", startedAt);
    logDatabaseStartupTerminal(failed);
    if (!win.isDestroyed() && !win.webContents.isDestroyed()) {
      win.webContents.send(InternalChannels.DatabaseStartupState, failed);
    }
    throw error;
  }
  let disposed = false;
  let state = startupState(startupId, attemptId, 1, "ready", startedAt);
  logDatabaseStartupTerminal(state);

  const forward = (next: DatabaseStartupState) => {
    if (disposed || win.isDestroyed() || win.webContents.isDestroyed()) return;
    state = next;
    win.webContents.send(InternalChannels.DatabaseStartupState, next);
  };
  const control = (event: Electron.IpcMainEvent, raw: unknown) => {
    if (disposed || event.sender !== win.webContents) return;
    const parsed = databaseStartupControlSchema.safeParse(raw);
    if (!parsed.success) return;
    if (parsed.data.action === "exit") {
      requestDatabaseStartupQuit();
      return;
    }
    if (parsed.data.action === "snapshot") forward(state);
  };

  const dispose = () => {
    if (disposed) return;
    disposed = true;
    ipcMain.removeListener(InternalChannels.DatabaseStartupControl, control);
    win.removeListener("closed", dispose);
    attachment.dispose();
  };

  ipcMain.on(InternalChannels.DatabaseStartupControl, control);
  win.once("closed", dispose);
  attachment.onLost(() => {
    if (disposed) return;
    forward(startupState(startupId, attemptId, state.sequence + 1, "failed", startedAt));
    options.onLost?.();
  });

  if (!win.isDestroyed() && !win.webContents.isDestroyed()) {
    win.webContents.postMessage(InternalChannels.ServicePort, { databaseStartupId: startupId }, [
      attachment.port as Electron.MessagePortMain,
    ]);
    forward(state);
    logger.info(`[createWindow] attached renderer to Rust Host (startupId=${startupId})`);
  }

  return { startupId, dispose };
}
