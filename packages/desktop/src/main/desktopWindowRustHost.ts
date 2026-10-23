/**
 * `desktopWindowLifecycle` 的 Rust Host 分支（从主文件拆出以满足 max-lines）。
 * 负责窗口级 login-shell env patch、workspace 选择与 attachment 创建。
 */

import type { BrowserWindow } from "electron";
import { attachRustHostWindow, type RustWindowAttachment } from "./desktopRustHostWindow.js";
import type { RustHostManager } from "./rustHostManager.js";
import type { StartupWorkspaceWarmupTarget } from "./startupWorkspace.js";

const DEFAULT_WAIT_TIMEOUT_MS = 4_500;

export async function attachRustWindowHostForDomReady(options: {
  win: BrowserWindow;
  label: string;
  manager: RustHostManager;
  logger: { info: (...args: unknown[]) => void; warn: (...args: unknown[]) => void };
  agentWarmupTargets?: readonly StartupWorkspaceWarmupTarget[];
  runtimeProcessEnvPatchPromise?: Promise<Record<string, string>>;
  runtimeProcessEnvFallbackPatch: Record<string, string>;
  runtimeProcessEnvWaitTimeoutMs?: number;
  onLost?: () => void;
}): Promise<RustWindowAttachment> {
  const patch = await Promise.race([
    options.runtimeProcessEnvPatchPromise?.catch(() => options.runtimeProcessEnvFallbackPatch) ??
      Promise.resolve(options.runtimeProcessEnvFallbackPatch),
    new Promise<Record<string, string>>((resolve) => {
      setTimeout(
        () => resolve(options.runtimeProcessEnvFallbackPatch),
        options.runtimeProcessEnvWaitTimeoutMs ?? DEFAULT_WAIT_TIMEOUT_MS,
      );
    }),
  ]);
  const primary = options.agentWarmupTargets?.[0];
  return attachRustHostWindow({
    win: options.win,
    manager: options.manager,
    logger: options.logger,
    windowId: options.label,
    workspace: primary
      ? {
          path: primary.workspacePath,
          ...(primary.workspaceIdentity ? { identity: primary.workspaceIdentity } : {}),
        }
      : undefined,
    runtimeEnvPatch: patch,
    ...(options.onLost ? { onLost: options.onLost } : {}),
  });
}
