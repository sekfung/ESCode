/**
 * 抓包后端（docs/specs/usb-agent-tools.md §4）。
 *
 * 只有**一个**后端：Windows 上随安装包提供的 `escode-usb-capture` 服务（安装时一并装上 USBPcap 驱动）。
 * Linux 与 macOS 不做实时抓包——它们的路径是**离线解码**：用户把在 Windows 机器或硬件分析仪上抓到的
 * `.pcapng` 交给 `usb_decode`。
 *
 * 这里刻意**不探测任何用户自装的第三方抓包工具**（曾经保留的 dumpcap 回退已按产品决定删除）：
 * 「装个 Wireshark 就能抓」会变成一条隐性依赖，而我们要的是「装完 ESCode 就能抓，装不了就明确告诉用户
 * 去别的机器抓文件」。Linux 上如果确实存在自建的抓包器（apps/usb-capture-helper），也会被认出来并使用，
 * 但那不是产品路径、也不给任何安装承诺。
 */

import { access, constants } from "node:fs";
import { join } from "node:path";
import { execFile } from "node:child_process";
import { promisify } from "node:util";

const run = promisify(execFile);
const PROBE_TIMEOUT_MS = 10_000;

export type CaptureBackendId = "builtin";

export interface CaptureInterfaceRef {
  id: string;
  bus?: number;
  label: string;
}

export interface BuiltinProbe {
  available: boolean;
  usbmonModuleLoaded: boolean;
  privilege: "ok" | "denied" | "unknown";
  interfaces: CaptureInterfaceRef[];
  reason?: string;
  remediation?: string;
}

export interface CaptureBackendStatus {
  id: CaptureBackendId;
  available: boolean;
  command?: string;
  privilege: "ok" | "denied" | "unknown";
  interfaces: CaptureInterfaceRef[];
  reason?: string;
  remediation?: string;
  helper?: BuiltinProbe;
}

export interface CaptureBackends {
  /** 实际抓包会用的后端；没有可用后端时为 undefined。 */
  preferred?: CaptureBackendId;
  backends: CaptureBackendStatus[];
}

export interface BackendProbeInput {
  env: Record<string, string | undefined>;
  platform?: NodeJS.Platform;
}

async function isExecutable(path: string): Promise<boolean> {
  return new Promise((resolve) => {
    access(path, constants.X_OK, (error) => resolve(!error));
  });
}

/** 平台资源目录：`<pluginRoot>/bin/<platform>-<arch>/`，与打包链路的投放约定一致。 */
function platformBinDirectory(platform: NodeJS.Platform, arch: string): string {
  return `bin/${platform}-${arch}`;
}

/**
 * 解析抓包器路径。三级回退，顺序即优先级：
 * 1. env 显式覆盖（开发与测试用）；
 * 2. 插件包内 `bin/<platform>-<arch>/`（官方插件 seed 会带着它，安装包无需额外配置）；
 * 3. Electron 资源目录的 `tools/`（与 cua-helper 同构的投放位置）。
 */
export function resolveHelperCandidates(input: {
  env: Record<string, string | undefined>;
  platform?: NodeJS.Platform;
  arch?: string;
  resourcesPath?: string;
}): string[] {
  const platform = input.platform ?? process.platform;
  const arch = input.arch ?? process.arch;
  const executable = platform === "win32" ? "escode-usb-capture.exe" : "escode-usb-capture";
  const candidates: string[] = [];
  const override = input.env.USB_PLUGIN_CAPTURE_HELPER_PATH?.trim();
  if (override) candidates.push(override);
  const pluginRoot = input.env.ESCODE_PLUGIN_ROOT?.trim();
  if (pluginRoot) {
    candidates.push(join(pluginRoot, platformBinDirectory(platform, arch), executable));
  }
  // `resourcesPath` 是 Electron 注入的属性，@types/node 的 Process 里没有（与 desktop 侧同样用交叉类型读）。
  const resourcesPath =
    input.resourcesPath ??
    (process as NodeJS.Process & { resourcesPath?: string }).resourcesPath;
  if (resourcesPath) candidates.push(join(resourcesPath, "tools", executable));
  return candidates;
}

/** 取第一个真实存在且可执行的候选；都没有时返回 undefined 与用于报错的候选清单。 */
export async function resolveHelperPath(
  input: Parameters<typeof resolveHelperCandidates>[0],
): Promise<{ path?: string; tried: string[] }> {
  const tried = resolveHelperCandidates(input);
  for (const candidate of tried) {
    if (await isExecutable(candidate)) return { path: candidate, tried };
  }
  return { tried };
}

/** 调用抓包器的 `--probe`，把它的自报结果当成后端状态。 */
export async function probeBuiltinHelper(
  helperPath: string,
  env: Record<string, string | undefined>,
): Promise<CaptureBackendStatus> {
  try {
    const { stdout } = await run(helperPath, ["--probe"], { timeout: PROBE_TIMEOUT_MS, env });
    const parsed: unknown = JSON.parse(stdout.trim().split("\n").pop() ?? "{}");
    const record = (parsed ?? {}) as Record<string, unknown>;
    if (record.ok !== true) {
      const error = (record.error ?? {}) as Record<string, unknown>;
      return {
        id: "builtin",
        available: false,
        command: helperPath,
        privilege: "denied",
        interfaces: [],
        ...(typeof error.message === "string" ? { reason: error.message } : {}),
        ...(typeof error.remediation === "string" ? { remediation: error.remediation } : {}),
      };
    }
    const interfaces = Array.isArray(record.interfaces)
      ? record.interfaces.flatMap((entry) => {
          const item = (entry ?? {}) as Record<string, unknown>;
          if (typeof item.id !== "string") return [];
          return [
            {
              id: item.id,
              label: typeof item.label === "string" ? item.label : item.id,
              ...(typeof item.bus === "number" ? { bus: item.bus } : {}),
            },
          ];
        })
      : [];
    const privilege =
      record.privilege === "ok" ? "ok" : record.privilege === "denied" ? "denied" : "unknown";
    const helper: BuiltinProbe = {
      available: record.available === true,
      usbmonModuleLoaded: record.usbmonModuleLoaded === true,
      privilege,
      interfaces,
      ...(typeof record.reason === "string" ? { reason: record.reason } : {}),
      ...(typeof record.remediation === "string" ? { remediation: record.remediation } : {}),
    };
    return {
      id: "builtin",
      available: helper.available,
      command: helperPath,
      privilege,
      interfaces,
      helper,
      ...(helper.reason ? { reason: helper.reason } : {}),
      ...(helper.remediation ? { remediation: helper.remediation } : {}),
    };
  } catch (error) {
    return {
      id: "builtin",
      available: false,
      command: helperPath,
      privilege: "unknown",
      interfaces: [],
      reason: `抓包器无法执行：${error instanceof Error ? error.message : String(error)}`,
      remediation: "用最新安装包重装 ESCode，或用 USB_PLUGIN_CAPTURE_HELPER_PATH 指向可用的抓包器排查。",
    };
  }
}

export async function resolveCaptureBackends(
  input: BackendProbeInput,
): Promise<CaptureBackends> {
  const platform = input.platform ?? process.platform;
  const backends: CaptureBackendStatus[] = [];

  if (platform === "win32") {
    // Windows 是唯一的实时抓包平台：内置服务随安装包注册，缺失时引导重装。
    const resolved = await resolveHelperPath({ env: input.env, platform });
    if (resolved.path) {
      backends.push(await probeBuiltinHelper(resolved.path, input.env));
    } else {
      backends.push({
        id: "builtin",
        available: false,
        privilege: "unknown",
        interfaces: [],
        reason: `没有找到 escode-usb-capture 服务（随安装包注册；路径候选：${resolved.tried.join("、") || "无"}）。`,
        remediation:
          "用最新安装包重装（安装时会注册 USB 抓包服务与 USBPcap 驱动）——在装好之前请让用户在别的机器上抓包，把 .pcapng 交给我离线分析。",
      });
    }
  } else if (platform === "linux") {
    // Linux 不在实时抓包的产品路径上：只有抓包器**真的存在**时才登记（例如用户自建了
    // apps/usb-capture-helper）。不存在就不列出来，也不给任何「重装就能用」的暗示。
    const resolved = await resolveHelperPath({ env: input.env, platform });
    if (resolved.path) backends.push(await probeBuiltinHelper(resolved.path, input.env));
  }

  const preferred = backends.find((backend) => backend.available)?.id;
  return { ...(preferred ? { preferred } : {}), backends };
}

/** 抓包时选择后端；都不可用时把**首选候选**的错误信息交出去（它比调用方更清楚缺什么）。 */
export function selectCaptureBackend(backends: CaptureBackends): {
  backend?: CaptureBackendStatus;
  failure?: CaptureBackendStatus;
} {
  const usable = backends.backends.find((backend) => backend.available);
  if (usable) return { backend: usable };
  const first = backends.backends[0];
  return first ? { failure: first } : {};
}
