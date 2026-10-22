//! Computer Use Helper 权限服务的共享实现。
//!
//! Node Host（`node.ts`）与 Electron Main（`desktopCuaPermissionService.ts`）共用这份编排：
//! 探测/拉起/重启 Helper、读取权限真值、执行 screen-capture 探针。两边只在下面注入的
//! Helper 生命周期依赖上不同，避免出现两条行为路径。
//!
//! 规格：`docs/specs/rust-u2-cua-permission.md`。

import {
  buildHelperOpenArgs,
  isCuaLocalDevelopmentRuntime,
} from "@escode/escode-cua/broker/server";
import { callBrokerMethod } from "@escode/escode-cua/broker/helperHealth";
import { resolveBrokerSocketPath } from "@escode/escode-cua/broker/socketPath";
import {
  isScreenCaptureProbeSuccess,
  resolveStandaloneStatusLaunchCandidates,
  shouldRunCuaScreenCaptureProbe,
  type CuaHelperHost,
  type CuaPermissionRestartOptions,
  type CuaPermissionRestartResult,
  type CuaPermissionState,
  type CuaPermissionStatusQueryOptions,
  type CuaPermissionStatusResult,
  type CuaProductMcpServerResolver,
  type CuaProductMcpServerResolverContext,
  type ManagedCuaProductHelperHost,
} from "./index.js";
import { getStableCuaPipeName } from "./stableCuaTransport.js";

/** `createDefaultCuaProductHelper` 的返回形状（Node Host 与 Main 共用）。 */
export type CuaPermissionHelper = {
  host: ManagedCuaProductHelperHost;
  macPermissionHost?: CuaHelperHost;
  resolver: CuaProductMcpServerResolver;
};

export interface CuaPermissionServiceDeps {
  platform?: NodeJS.Platform;
  env?: NodeJS.ProcessEnv;
  /** 当前 workspace/context 是否启用 Computer Use（与 Node Host 的 `isCuaEnabledForContext` 同义）。 */
  isCuaEnabled: (context?: CuaProductMcpServerResolverContext) => boolean;
  /** 当前托管的 Helper（无则 undefined）。 */
  peekHelper: () => CuaPermissionHelper | undefined;
  /** 按需获取/创建托管 Helper。 */
  getOrCreateHelper: (
    context?: CuaProductMcpServerResolverContext,
  ) => Promise<CuaPermissionHelper | undefined>;
  /** 代际校验：Helper 在 await 期间被 dispose 后不得继续返回其结果。 */
  isCurrent: (helper: CuaPermissionHelper) => boolean;
}

/** 只 ping 稳定 socket，绝不拉起 Helper；300ms 预算。 */
export async function probeStableCuaHelperSocket(
  platform: NodeJS.Platform = process.platform,
): Promise<string | null> {
  const socketPath = platform === "win32" ? getStableCuaPipeName() : resolveBrokerSocketPath();
  try {
    const { createConnection } = await import("node:net");
    return await new Promise<string | null>((resolve) => {
      const socket = createConnection(socketPath);
      const finish = (value: string | null): void => {
        socket.destroy();
        resolve(value);
      };
      const timer = setTimeout(() => finish(null), 300);
      socket.on("connect", () => {
        clearTimeout(timer);
        socket.write(`{"id":0,"method":"ping","params":{}}\n`);
        let buffer = "";
        socket.on("data", (chunk: Buffer) => {
          buffer += chunk.toString("utf8");
          if (buffer.includes("\n")) {
            try {
              const nl = buffer.indexOf("\n");
              const reply = JSON.parse(buffer.slice(0, nl)) as { ok?: boolean };
              finish(reply.ok === true ? socketPath : null);
            } catch {
              finish(null);
            }
          }
        });
      });
      socket.on("error", () => {
        clearTimeout(timer);
        finish(null);
      });
    });
  } catch {
    return null;
  }
}

/** macOS 设置页只读探测：拉起 standalone Helper（稳定 socket，无 launcher-pid）。 */
export async function launchStandaloneCuaHelperForStatus(
  platform: NodeJS.Platform = process.platform,
  env: NodeJS.ProcessEnv = process.env,
): Promise<string | null> {
  if (platform !== "darwin") return null;
  const { existsSync } = await import("node:fs");
  const { execFile } = await import("node:child_process");
  const { promisify } = await import("node:util");
  const socketPath = resolveBrokerSocketPath();
  const appPath = resolveStandaloneStatusLaunchCandidates(env).find((candidate) =>
    existsSync(candidate),
  );
  if (!appPath) return null;
  const args = buildHelperOpenArgs({
    appPath,
    socketPath,
    exitLogPath: `${socketPath}.settings.exit.log`,
    ...(isCuaLocalDevelopmentRuntime(env)
      ? { allowUnsignedLauncherLocalDev: true, allowExternalBrokerClientLocalDev: true }
      : {}),
  });
  try {
    await promisify(execFile)("/usr/bin/open", args, { timeout: 5_000 });
  } catch {
    // LaunchServices 已接单也可能超时；继续等 ping。
  }
  for (let attempt = 0; attempt < 50; attempt += 1) {
    const ready = await probeStableCuaHelperSocket(platform);
    if (ready) return ready;
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  return null;
}

/** Windows 设置页探测：启动产品 Helper 后轮询稳定 pipe。 */
export async function launchWindowsCuaHelperForStatus(
  getOrCreateHelper: () => Promise<CuaPermissionHelper | undefined>,
  platform: NodeJS.Platform = process.platform,
): Promise<string | null> {
  try {
    const managed = await getOrCreateHelper();
    if (managed) void managed.host.start().catch(() => {});
  } catch {}
  for (let attempt = 0; attempt < 50; attempt += 1) {
    const ready = await probeStableCuaHelperSocket(platform);
    if (ready) return ready;
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  return null;
}

export async function runCuaScreenCaptureReadinessProbe(
  host: Pick<CuaHelperHost, "queryScreenCaptureProbe">,
  screenRecording: "granted" | "denied" | "unknown",
  queryOptions?: CuaPermissionStatusQueryOptions,
): Promise<boolean> {
  if (!shouldRunCuaScreenCaptureProbe(screenRecording, queryOptions)) {
    return false;
  }
  try {
    return isScreenCaptureProbeSuccess(await host.queryScreenCaptureProbe());
  } catch {
    return false;
  }
}

/** Screen Recording 展示态：优先短命 Helper 的 TCC 真值，失败沿用常驻 Helper 报告。 */
export async function resolveCuaScreenRecordingState(
  host: Pick<CuaHelperHost, "queryScreenRecordingPreflight">,
  reported: "granted" | "denied" | "unknown",
): Promise<"granted" | "denied" | "unknown"> {
  if (reported === "unknown") return reported;
  try {
    return (await host.queryScreenRecordingPreflight()) ?? reported;
  } catch {
    return reported;
  }
}

export function createCuaPermissionService(
  deps: CuaPermissionServiceDeps,
): import("./index.js").ICuaPermissionService {
  const platform = deps.platform ?? process.platform;

  return {
    async getStatus(
      workspacePath: string,
      workspaceIdentity?: string,
      queryOptions?: CuaPermissionStatusQueryOptions,
    ): Promise<CuaPermissionStatusResult> {
      if (platform !== "darwin" && platform !== "win32") {
        return {
          available: false,
          reason: "CUA permissions are only available on macOS and Windows.",
        };
      }
      const context = workspacePath ? { workspacePath, workspaceIdentity } : undefined;
      // 插件关闭只门控后续 Agent；权限页刷新不得进入 acquire，否则会把仍被已有 Agent 使用的 Helper 停掉。
      if (!deps.isCuaEnabled(context)) {
        return {
          available: false,
          reason: "ESCode Computer Use is not enabled (plugin off or not product mode).",
        };
      }
      const helper = deps.peekHelper();
      const host = helper ? helper.macPermissionHost : undefined;
      if (!host || !host.running) {
        let stable = await probeStableCuaHelperSocket(platform);
        if (!stable) {
          stable =
            platform === "darwin"
              ? await launchStandaloneCuaHelperForStatus(platform, deps.env)
              : await launchWindowsCuaHelperForStatus(() => deps.getOrCreateHelper(), platform);
        }
        if (!stable) {
          return {
            available: false,
            reason:
              "ESCode Computer Use is not running; it will start automatically on first Computer Use use.",
            idle: true,
          } satisfies { available: false; reason: string; idle: true };
        }
        try {
          const report = await callBrokerMethod<{
            grant_owner: string | null;
            owner?: { display_name?: string };
            accessibility: CuaPermissionState;
            accessibility_probe_ok?: boolean;
            screen_recording: CuaPermissionState;
          }>({
            socketPath: stable,
            method: "permission_status",
            timeoutMs: 3000,
          });
          return {
            grantOwner: report.grant_owner,
            grantOwnerDisplayName: report.owner?.display_name ?? report.grant_owner,
            accessibility: report.accessibility,
            accessibilityProbeOk: report.accessibility_probe_ok === true,
            screenRecording: report.screen_recording,
            screenCaptureProbeOk: false,
          };
        } catch {
          return {
            available: false,
            reason: "ESCode Computer Use is starting up; retry in a moment.",
            idle: true,
          } satisfies { available: false; reason: string; idle: true };
        }
      }
      try {
        const report = await host.queryPermissionStatus();
        if (!helper || !deps.isCurrent(helper)) {
          return {
            available: false,
            reason: "ESCode Computer Use lifecycle is disposed.",
          };
        }
        const screenRecording = await resolveCuaScreenRecordingState(host, report.screen_recording);
        if (!deps.isCurrent(helper)) {
          return {
            available: false,
            reason: "ESCode Computer Use lifecycle is disposed.",
          };
        }
        const screenCaptureProbeOk = await runCuaScreenCaptureReadinessProbe(
          host,
          screenRecording,
          queryOptions,
        );
        if (!deps.isCurrent(helper)) {
          return {
            available: false,
            reason: "ESCode Computer Use lifecycle is disposed.",
          };
        }
        const reportedOwnerDisplayName =
          typeof report.owner?.display_name === "string" ? report.owner.display_name : undefined;
        return {
          grantOwner: report.grant_owner,
          grantOwnerDisplayName: reportedOwnerDisplayName ?? report.grant_owner,
          accessibility: report.accessibility,
          accessibilityProbeOk:
            report.accessibility_probe?.ok === true &&
            report.accessibility_probe?.classification === "functional",
          screenRecording,
          screenCaptureProbeOk,
        };
      } catch (error) {
        return {
          available: false,
          reason: `Could not read Computer Use Helper permission status: ${
            error instanceof Error ? error.message : String(error)
          }`,
        };
      }
    },
    async restartHelper(
      workspacePath?: string,
      workspaceIdentity?: string,
      restartOptions?: CuaPermissionRestartOptions,
    ): Promise<CuaPermissionRestartResult> {
      if (platform !== "darwin") {
        return {
          ok: false,
          reason: "CUA permissions are only available on macOS.",
        };
      }
      const context = workspacePath ? { workspacePath, workspaceIdentity } : undefined;
      if (!deps.isCuaEnabled(context)) {
        return {
          ok: false,
          reason: "ESCode Computer Use is not enabled (plugin off or not product mode).",
        };
      }
      const helper = await deps.getOrCreateHelper(context);
      const resolver =
        helper && helper.macPermissionHost && deps.isCurrent(helper) ? helper.resolver : undefined;
      if (!resolver) {
        return {
          ok: false,
          reason: "ESCode Computer Use is not enabled (plugin off or not product mode).",
        };
      }
      try {
        if (restartOptions?.reason === "permission_granted") {
          await resolver.restartAfterPermissionGrant(restartOptions.onboardingSessionId);
        } else {
          await resolver.restart();
        }
        if (!helper || !deps.isCurrent(helper)) {
          return {
            ok: false,
            reason: "ESCode Computer Use lifecycle is disposed.",
          };
        }
        return { ok: true };
      } catch (error) {
        return {
          ok: false,
          reason: `Failed to restart ESCode Computer Use: ${
            error instanceof Error ? error.message : String(error)
          }`,
        };
      }
    },
  };
}
