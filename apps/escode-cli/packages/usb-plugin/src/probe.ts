/**
 * 环境探测（docs/specs/usb-agent-tools.md §3.2）。
 *
 * 只做**只读**探测：本机能不能抓、用什么后端、为什么不能抓（给可执行的 remediation）、
 * 接了哪些设备、已经有哪些抓包文件。任何提权动作都不在这里做——把命令告诉用户，让他自己决定。
 */

import { readdir, readFile, stat } from "node:fs/promises";
import { join } from "node:path";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import {
  resolveCaptureBackends,
  type CaptureBackendId,
  type CaptureBackendStatus,
  type CaptureInterfaceRef,
} from "./backends.js";
import { interfaceClassName, protocolsForInterfaceClass, type ProtocolName } from "./types.js";

const run = promisify(execFile);
const PROBE_TIMEOUT_MS = 10_000;

export interface ProbeDevice {
  busDev?: string;
  bus?: number;
  device?: number;
  vid?: string;
  pid?: string;
  manufacturer?: string;
  product?: string;
  serialNumber?: string;
  interfaces: Array<{ classCode: number; className: string; protocols: ProtocolName[] }>;
  /** 平台无法给出总线地址时的说明（例如 Windows）。 */
  addressNote?: string;
}

/** 顶层字段取自**首选**后端，便于调用方直接展示；完整候选在 `backends` 里。 */
export interface ProbeCapture {
  available: boolean;
  backend: CaptureBackendId | "none";
  preferred?: CaptureBackendId;
  command?: string;
  interfaces: CaptureInterfaceRef[];
  privilege: "ok" | "denied" | "unknown";
  reason?: string;
  remediation?: string;
  backends: CaptureBackendStatus[];
}

export interface ProbeResult {
  platform: NodeJS.Platform;
  capture: ProbeCapture;
  devices: ProbeDevice[];
  captures: Array<{ path: string; bytes: number; modifiedAt: string }>;
  notes: string[];
}

export interface ProbeInput {
  env: Record<string, string | undefined>;
  workspace: string;
  pluginData: string;
}

/** 抓包后端探测已移到 backends.ts（builtin 优先、dumpcap 回退）；这里只做环境清单。 */


/** Linux：/sys/bus/usb/devices 不需要特权，接口类目都在这里。 */
async function probeLinuxDevices(): Promise<ProbeDevice[]> {
  const root = "/sys/bus/usb/devices";
  let entries: string[];
  try {
    entries = await readdir(root);
  } catch {
    return [];
  }
  const devices: ProbeDevice[] = [];
  for (const entry of entries) {
    // 只有形如 1-4 或 1-4.2 的设备目录（父设备或子设备）带 idVendor；接口目录形如 1-4:1.0。
    if (entry.includes(":")) continue;
    const dir = join(root, entry);
    const read = async (name: string): Promise<string | undefined> => {
      try {
        return (await readFile(join(dir, name), "utf8")).trim();
      } catch {
        return undefined;
      }
    };
    const vid = await read("idVendor");
    if (!vid) continue;
    const busnum = Number(await read("busnum"));
    const devnum = Number(await read("devnum"));
    const interfaces: ProbeDevice["interfaces"] = [];
    for (const child of await readdir(dir).catch(() => [])) {
      if (!child.includes(":")) continue;
      const classCode = Number(
        await readFile(join(dir, child, "bInterfaceClass"), "utf8").catch(() => ""),
      );
      if (!Number.isFinite(classCode)) continue;
      interfaces.push({
        classCode,
        className: interfaceClassName(classCode),
        protocols: protocolsForInterfaceClass(classCode),
      });
    }
    const manufacturer = await read("manufacturer");
    const product = await read("product");
    const serialNumber = await read("serial");
    devices.push({
      ...(Number.isFinite(busnum) && Number.isFinite(devnum)
        ? { busDev: `${busnum}-${devnum}`, bus: busnum, device: devnum }
        : {}),
      vid,
      pid: await read("idProduct"),
      ...(manufacturer ? { manufacturer } : {}),
      ...(product ? { product } : {}),
      ...(serialNumber ? { serialNumber } : {}),
      interfaces,
    });
  }
  return devices;
}

/** Windows：PnP 只给 VID:PID 与实例 ID，没有总线地址——这一点必须如实说明，别让模型去对号。 */
async function probeWindowsDevices(): Promise<ProbeDevice[]> {
  try {
    const { stdout } = await run(
      "powershell",
      [
        "-NoProfile",
        "-Command",
        "Get-PnpDevice -PresentOnly | Where-Object { $_.InstanceId -like 'USB\\*' } | Select-Object Class,FriendlyName,InstanceId | ConvertTo-Json -Compress",
      ],
      { timeout: PROBE_TIMEOUT_MS, maxBuffer: 4 * 1024 * 1024 },
    );
    const parsed: unknown = JSON.parse(stdout || "[]");
    const rows = Array.isArray(parsed) ? parsed : [parsed];
    return rows.flatMap((row) => {
      if (!row || typeof row !== "object") return [];
      const record = row as Record<string, unknown>;
      const instanceId = typeof record.InstanceId === "string" ? record.InstanceId : "";
      const vid = /VID_([0-9A-F]{4})/u.exec(instanceId)?.[1];
      const pid = /PID_([0-9A-F]{4})/u.exec(instanceId)?.[1];
      const name = typeof record.FriendlyName === "string" ? record.FriendlyName : undefined;
      if (!vid) return [];
      return [
        {
          vid: vid.toLowerCase(),
          ...(pid ? { pid: pid.toLowerCase() } : {}),
          ...(name ? { product: name } : {}),
          interfaces: [],
          addressNote: "Windows 不提供总线地址；抓包后在 summary 里按 VID:PID 辨认设备。",
        },
      ];
    });
  } catch {
    return [];
  }
}

/** macOS：没有任何内核抓包接口，设备清单仅供参考（分析要靠别的机器或硬件分析仪）。 */
async function probeMacDevices(): Promise<ProbeDevice[]> {
  try {
    const { stdout } = await run("system_profiler", ["SPUSBDataType", "-json"], {
      timeout: 30_000,
      maxBuffer: 8 * 1024 * 1024,
    });
    const parsed = JSON.parse(stdout) as { SPUSBDataType?: unknown };
    const devices: ProbeDevice[] = [];
    const walk = (node: unknown): void => {
      if (Array.isArray(node)) {
        node.forEach(walk);
        return;
      }
      if (!node || typeof node !== "object") return;
      const record = node as Record<string, unknown>;
      const name = typeof record._name === "string" ? record._name : undefined;
      const vid = typeof record.vendor_id === "string" ? record.vendor_id : undefined;
      const pid = typeof record.product_id === "string" ? record.product_id : undefined;
      if (name || vid) {
        devices.push({
          ...(vid ? { vid: vid.split(" ")[0]?.toLowerCase() } : {}),
          ...(pid ? { pid: pid.split(" ")[0]?.toLowerCase() } : {}),
          ...(name ? { product: name } : {}),
          interfaces: [],
          addressNote: "macOS 不提供总线地址，也无法实时抓包。",
        });
      }
      walk(record._items);
    };
    walk(parsed.SPUSBDataType);
    return devices;
  } catch {
    return [];
  }
}

/** 抓包文件清单：插件数据目录 + 工作区 `.escode/usb-captures`，最近修改的在前。 */
export async function listCaptureFiles(input: {
  workspace: string;
  pluginData: string;
  limit?: number;
}): Promise<Array<{ path: string; bytes: number; modifiedAt: string }>> {
  const directories = [join(input.pluginData, "captures"), join(input.workspace, ".escode", "usb-captures")];
  const files: Array<{ path: string; bytes: number; modifiedAt: string }> = [];
  for (const directory of directories) {
    for (const name of await readdir(directory).catch(() => [])) {
      if (!/\.(pcapng|pcap)$/iu.test(name)) continue;
      const path = join(directory, name);
      const info = await stat(path).catch(() => undefined);
      if (!info?.isFile()) continue;
      files.push({ path, bytes: info.size, modifiedAt: info.mtime.toISOString() });
    }
  }
  files.sort((left, right) => right.modifiedAt.localeCompare(left.modifiedAt));
  return files.slice(0, input.limit ?? 20);
}

export async function probeEnvironment(input: ProbeInput): Promise<ProbeResult> {
  const notes: string[] = [];
  const platform = process.platform;
  const backends = await resolveCaptureBackends({ env: input.env, platform });
  // 顶层字段取自「首选且可用」的后端；一个都不可用时回落到第一个候选，
  // 这样 reason/remediation 仍然是「为什么首选路径不能抓」而不是一句没有信息的“没有后端”。
  const usable = backends.backends.find((backend) => backend.available);
  const headline = usable ?? backends.backends[0];
  const capture: ProbeCapture = {
    available: usable !== undefined,
    backend: usable?.id ?? "none",
    ...(backends.preferred ? { preferred: backends.preferred } : {}),
    interfaces: headline?.interfaces ?? [],
    privilege: headline?.privilege ?? "unknown",
    ...(headline?.command ? { command: headline.command } : {}),
    ...(headline?.reason ? { reason: headline.reason } : {}),
    ...(headline?.remediation ? { remediation: headline.remediation } : {}),
    backends: backends.backends,
  };
  if (platform === "darwin") {
    // macOS 是明确的例外：没有后端，也没有「装点什么就能抓」的路径。
    capture.backends = [
      {
        id: "builtin",
        available: false,
        privilege: "unknown",
        interfaces: [],
        reason: "macOS 没有公开的内核级 USB 抓包接口。",
        remediation:
          "用硬件分析仪（Lecroy / Beagle / Total Phase）或另一台 Linux/Windows 机器抓包，" +
          "把 pcapng 文件交给我用 usb_decode 分析；离线分析在本机完全可用。",
      },
    ];
  }

  if (capture.available && capture.privilege !== "ok") {
    // 后端存在但没权限：available 仍为 true（接口可枚举），由 remediation 说明 usb_capture 为什么会失败——
    // 这比直接说「不可用」更有用，同时提醒模型不要编造抓包结果。
    notes.push("抓包后端存在但权限不足：usb_capture 会返回 permission 与 remediation，不要编造抓包结果。");
  }
  const unavailable = backends.backends.filter((backend) => !backend.available);
  if (!capture.available && unavailable.length > 1) {
    notes.push(`两个抓包后端都不可用：${unavailable.map((backend) => `${backend.id}（${backend.reason ?? "原因未知"}）`).join("；")}`);
  }

  const devices =
    platform === "linux"
      ? await probeLinuxDevices()
      : platform === "win32"
        ? await probeWindowsDevices()
        : platform === "darwin"
          ? await probeMacDevices()
          : [];
  const captures = await listCaptureFiles({
    workspace: input.workspace,
    pluginData: input.pluginData,
  });
  if (devices.length === 0) {
    notes.push("没有枚举到 USB 设备：可能是容器/无设备环境，或枚举命令不可用。");
  }
  return { platform, capture, devices, captures, notes };
}
