/**
 * Main 侧的 Computer Use Helper 权限服务（docs/specs/rust-u2-cua-permission.md）。
 *
 * Helper 生命周期由 Main 拥有：Rust Host 只经控制通道 `mainRequest` 转发
 * `cua-permission.getStatus` / `restartHelper`。Node Host 与 Main 共用
 * `cuaPermissionServiceRuntime` 的编排，这里只负责 Main 的单例与进程环境。
 */

import { CuaHelperLifecycleManager } from "@escode/escode-cua/broker/server";
import { isESCodeCuaInternalFeatureEnabled } from "@escode/shared";
import {
  createCuaPermissionService,
  createDefaultCuaProductHelper,
  isOfficialCuaPluginEnabledForWorkspace,
  shouldCreateDefaultCuaProductHelper,
  shouldRetainDefaultCuaProductHelper,
  type CuaPermissionHelper,
} from "@escode/services/node";
import { resolvePackagedCuaHelperAppPath } from "./desktopCuaHelperInstaller.js";

type CuaPermissionService = ReturnType<typeof createCuaPermissionService>;
type ManagedHelper = { helper: CuaPermissionHelper };

let singleton:
  | { service: CuaPermissionService; lifecycle: CuaHelperLifecycleManager<ManagedHelper> }
  | undefined;

/**
 * Helper 的 launcher 必须是签名的 Electron 主进程；Node Host 过去在 spawn env 里注入，
 * Main 直接持有该进程，因此这里补到 `process.env`，供 producer 的 Helper 装配读取。
 */
function installCuaProcessEnv(): void {
  if (process.platform === "darwin") {
    process.env.ESCODE_CUA_LAUNCHER_PID ??= String(process.pid);
  }
  const bundled = resolvePackagedCuaHelperAppPath();
  if (bundled) {
    process.env.ESCODE_CUA_BUNDLED_HELPER_APP_PATH ??= bundled;
  }
}

function createService(): NonNullable<typeof singleton> {
  installCuaProcessEnv();
  const platform = process.platform;
  const env = process.env;
  const isCuaEnabled = (context?: {
    workspacePath?: string;
    workspaceIdentity?: string;
  }): boolean =>
    isESCodeCuaInternalFeatureEnabled(env) ||
    isOfficialCuaPluginEnabledForWorkspace({
      env,
      workingDirectory: context?.workspacePath,
    });
  const lifecycle = new CuaHelperLifecycleManager<ManagedHelper>(async (managed) => {
    await managed.helper.host.stop();
  });
  const getOrCreateHelper = async (context?: {
    workspacePath?: string;
    workspaceIdentity?: string;
  }): Promise<CuaPermissionHelper | undefined> => {
    const managed = await lifecycle.acquire({
      isAdmitted: () =>
        shouldCreateDefaultCuaProductHelper({
          serviceAuthorityMode: "desktop-local",
          hasRemoteWorkspaceIdentity: Boolean(context?.workspaceIdentity?.trim()),
          hasInjectedResolver: false,
          hasBuiltInCuaPlugin: isCuaEnabled(context),
        }),
      shouldRetainCurrent: shouldRetainDefaultCuaProductHelper,
      create: () => {
        // Main 看不到 Rust Host 内的活跃 CUA turn；差异已在 spec §4.1 登记。
        const helper = createDefaultCuaProductHelper({ hasActiveTurn: () => false });
        return helper ? { helper } : undefined;
      },
    });
    return managed?.helper;
  };
  const service = createCuaPermissionService({
    platform,
    env,
    isCuaEnabled,
    peekHelper: () => lifecycle.peek()?.helper,
    getOrCreateHelper,
    isCurrent: (helper) => lifecycle.peek()?.helper === helper,
  });
  return { service, lifecycle };
}

function cuaService(): CuaPermissionService {
  singleton ??= createService();
  return singleton.service;
}

interface CuaPermissionMainParams {
  workspacePath?: string;
  workspaceIdentity?: string;
  options?: Record<string, unknown>;
}

function parseParams(value: unknown): CuaPermissionMainParams {
  if (value === undefined) return {};
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new Error("cua-permission params must be an object");
  }
  const record = value as Record<string, unknown>;
  const workspacePath = record.workspacePath;
  const workspaceIdentity = record.workspaceIdentity;
  const options = record.options;
  if (workspacePath !== undefined && typeof workspacePath !== "string") {
    throw new Error("workspacePath must be a string");
  }
  if (workspaceIdentity !== undefined && typeof workspaceIdentity !== "string") {
    throw new Error("workspaceIdentity must be a string");
  }
  if (
    options !== undefined &&
    (typeof options !== "object" || options === null || Array.isArray(options))
  ) {
    throw new Error("options must be an object");
  }
  return {
    ...(typeof workspacePath === "string" && workspacePath.trim() ? { workspacePath } : {}),
    ...(typeof workspaceIdentity === "string" && workspaceIdentity.trim()
      ? { workspaceIdentity }
      : {}),
    ...(options ? { options: options as Record<string, unknown> } : {}),
  };
}

/** Rust Host `mainRequest` 入口；未知 method 返回 undefined 交给调用方兜底。 */
export function createCuaPermissionMainHandler(
  getService: () => CuaPermissionService,
): (method: string, params: unknown) => Promise<unknown> | undefined {
  return (method, params) => {
    if (method !== "cua-permission.getStatus" && method !== "cua-permission.restartHelper") {
      return undefined;
    }
    let parsed: CuaPermissionMainParams;
    try {
      parsed = parseParams(params);
    } catch (error) {
      return Promise.reject(error);
    }
    if (method === "cua-permission.getStatus") {
      if (!parsed.workspacePath) {
        return Promise.reject(new Error("cua-permission.getStatus requires workspacePath"));
      }
      return getService().getStatus(parsed.workspacePath, parsed.workspaceIdentity, parsed.options);
    }
    return getService().restartHelper(
      parsed.workspacePath,
      parsed.workspaceIdentity,
      parsed.options,
    );
  };
}

export const handleCuaPermissionMainRequest = createCuaPermissionMainHandler(cuaService);

/** App 退出屏障调用；重复调用安全。 */
export async function disposeDesktopCuaPermissionService(): Promise<void> {
  const current = singleton;
  singleton = undefined;
  await current?.lifecycle.dispose();
}
