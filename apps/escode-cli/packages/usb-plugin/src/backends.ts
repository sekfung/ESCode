/**
 * 抓包后端解析（docs/specs/usb-agent-tools.md §4）。
 *
 * 优先级：`builtin`（随包分发的自研抓包器）→ `dumpcap`（用户自装 Wireshark 时的兼容后端）→ 无。
 * 这一层只负责「谁能抓、为什么不能抓、怎么修」，不负责抓包本身。
 */

import { access, constants } from "node:fs";
import { join } from "node:path";
import { execFile } from "node:child_process";
import { promisify } from "node:util";

const run = promisify(execFile);
const PROBE_TIMEOUT_MS = 10_000;

export type CaptureBackendId = "builtin" | "dumpcap";

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
  /** 仅 `builtin`：helper 自报的探测结果（Windows 服务缺失时为 undefined）。 */
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

async function which(
  command: string,
  env: Record<string, string | undefined>,
): Promise<string | undefined> {
  for (const dir of (env.PATH ?? "").split(":")) {
    if (!dir) continue;
    const candidate = join(dir, command);
    if (await isExecutable(candidate)) return candidate;
  }
  return undefined;
}

/** 平台资源目录：`<pluginRoot>/bin/<platform>-<arch>/`，与打包链路的投放约定一致。 */
function platformBinDirectory(platform: NodeJS.Platform, arch: string): string {
  return `bin/${platform}-${arch}`;
}

/**
 * 解析自研抓包器路径。三级回退，顺序即优先级：
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
    candidates.push(join(pluginRoot, platformBinDirectory(platform, "*"), executable));
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
  const tried = resolveHelperCandidates(input).filter((candidate) => !candidate.includes("*"));
  for (const candidate of tried) {
    if (await isExecutable(candidate)) return { path: candidate, tried };
  }
  return { tried };
}

/** 调用 helper 的 `--probe`，把它的自报结果当成后端状态。 */
export async function probeBuiltinHelper(
  helperPath: string,
): Promise<CaptureBackendStatus> {
  try {
    const { stdout } = await run(helperPath, ["--probe"], { timeout: PROBE_TIMEOUT_MS });
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
      reason: `自研抓包器无法执行：${error instanceof Error ? error.message : String(error)}`,
      remediation: "重新安装 ESCode，或用 USB_PLUGIN_CAPTURE_HELPER_PATH 指向可用的抓包器排查。",
    };
  }
}

/** dumpcap 需要 cap_net_raw + cap_net_admin（或 wireshark 组、或 root）才能打开 usbmon。 */
async function probeDumpcap(command: string): Promise<CaptureBackendStatus> {
  let interfaces: CaptureInterfaceRef[] = [];
  try {
    const { stdout } = await run(command, ["-D"], { timeout: PROBE_TIMEOUT_MS });
    interfaces = stdout
      .split("\n")
      .map((line) => line.trim())
      .filter((line) => line.includes("usbmon"))
      .map((line) => {
        const match = /^(\d+)\.\s+(\S+)/u.exec(line);
        const id = match?.[2] ?? line;
        const bus = /usbmon(\d+)/u.exec(id)?.[1];
        return { id, label: line, ...(bus ? { bus: Number(bus) } : {}) };
      });
  } catch (error) {
    return {
      id: "dumpcap",
      available: false,
      command,
      privilege: "unknown",
      interfaces,
      reason: `dumpcap -D 执行失败：${error instanceof Error ? error.message : String(error)}`,
      remediation: `确认 ${command} 可用，然后重新运行 usb_probe。`,
    };
  }
  if (interfaces.length === 0) {
    return {
      id: "dumpcap",
      available: false,
      command,
      privilege: "denied",
      interfaces,
      reason: "dumpcap 没有列出任何 usbmon 接口。",
      remediation: "加载 usbmon 内核模块后重试：`sudo modprobe usbmon`。",
    };
  }
  let privileged = process.getuid?.() === 0;
  if (!privileged) {
    try {
      const { stdout } = await run("getcap", [command], { timeout: PROBE_TIMEOUT_MS });
      privileged = /cap_net_(raw|admin)/u.test(stdout);
    } catch {
      privileged = false;
    }
  }
  if (!privileged) {
    try {
      const { stdout } = await run("id", ["-nG"], { timeout: PROBE_TIMEOUT_MS });
      privileged = stdout.split(/\s+/u).includes("wireshark");
    } catch {
      privileged = false;
    }
  }
  if (privileged) {
    return { id: "dumpcap", available: true, command, privilege: "ok", interfaces };
  }
  return {
    id: "dumpcap",
    available: false,
    command,
    privilege: "denied",
    interfaces,
    reason: "dumpcap 存在，但当前用户没有抓包权限（需要 CAP_NET_RAW/CAP_NET_ADMIN 或 wireshark 组成员）。",
    remediation:
      `授予能力：sudo setcap cap_net_raw,cap_net_admin+eip ${command}` +
      "；或把用户加入 wireshark 组后重新登录（也可以直接用内置抓包器，见 usb_probe 的 builtin 后端）。",
  };
}

/**
 * USBPcapCMD 只作兼容后端保留：内置服务才是 Windows 的首选路径。
 * 服务未安装时这里返回不可用，并说明内置抓包能力随安装包提供。
 */
async function probeUsbpcap(command: string): Promise<CaptureBackendStatus> {
  return {
    id: "dumpcap",
    available: true,
    command,
    privilege: "unknown",
    interfaces: [],
    reason: "USBPcapCMD 存在；抓包需要管理员权限（内置服务可用时优先走内置服务）。",
  };
}

export async function resolveCaptureBackends(
  input: BackendProbeInput,
): Promise<CaptureBackends> {
  const platform = input.platform ?? process.platform;
  const backends: CaptureBackendStatus[] = [];

  if (platform === "linux" || platform === "win32") {
    const resolved = await resolveHelperPath({ env: input.env, platform });
    if (resolved.path) {
      backends.push(await probeBuiltinHelper(resolved.path));
    } else {
      const serviceName = platform === "win32" ? "escode-usb-capture 服务" : "自研抓包器";
      backends.push({
        id: "builtin",
        available: false,
        privilege: "unknown",
        interfaces: [],
        reason: `没有找到${serviceName}（随安装包提供，路径候选：${resolved.tried.join("、") || "无"}）。`,
        remediation:
          platform === "win32"
            ? "用最新安装包重装（安装时会注册 USB 抓包服务与 USBPcap 驱动），或临时用 dumpcap 兼容后端。"
            : "用最新安装包重装（安装时会投放自研抓包器并配置 usbmon 权限），或临时用 dumpcap 兼容后端。",
      });
    }
  }

  if (platform === "linux") {
    const command =
      (input.env.USB_PLUGIN_DUMPCAP_PATH?.trim() &&
      (await isExecutable(input.env.USB_PLUGIN_DUMPCAP_PATH.trim()))
        ? input.env.USB_PLUGIN_DUMPCAP_PATH.trim()
        : undefined) ?? (await which("dumpcap", input.env));
    if (command) backends.push(await probeDumpcap(command));
  } else if (platform === "win32") {
    const candidates = [
      input.env.USB_PLUGIN_USBPCAP_CMD_PATH?.trim(),
      await which("USBPcapCMD.exe", input.env),
      input.env.ProgramFiles ? join(input.env.ProgramFiles, "USBPcap", "USBPcapCMD.exe") : undefined,
      input.env["ProgramFiles(x86)"]
        ? join(input.env["ProgramFiles(x86)"], "Wireshark", "USBPcapCMD.exe")
        : undefined,
    ].filter((value): value is string => Boolean(value));
    for (const candidate of candidates) {
      if (await isExecutable(candidate)) {
        backends.push(await probeUsbpcap(candidate));
        break;
      }
    }
  }

  const preferred = backends.find((backend) => backend.available)?.id;
  return { ...(preferred ? { preferred } : {}), backends };
}

/** 抓包时选择后端：优先内置，其次 dumpcap；都不可用时用**首选不可用后端**的错误信息报错。 */
export function selectCaptureBackend(backends: CaptureBackends): {
  backend?: CaptureBackendStatus;
  failure?: CaptureBackendStatus;
} {
  const usable = backends.backends.find((backend) => backend.available);
  if (usable) return { backend: usable };
  const first = backends.backends[0];
  return first ? { failure: first } : {};
}
