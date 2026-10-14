/**
 * 环境探测（docs/specs/usb-agent-tools.md §3.2）。
 *
 * 只做**只读**探测：本机能不能抓、用什么后端、为什么不能抓（给可执行的 remediation）、
 * 接了哪些设备、已经有哪些抓包文件。任何提权动作都不在这里做——把命令告诉用户，让他自己决定。
 */

import { access, readdir, readFile, stat } from "node:fs/promises";
import { constants } from "node:fs";
import { join } from "node:path";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
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

export interface ProbeCapture {
  available: boolean;
  backend: "dumpcap" | "usbpcap" | "none";
  command?: string;
  interfaces: Array<{ id: string; bus?: number; label: string }>;
  privilege: "ok" | "denied" | "unknown";
  reason?: string;
  remediation?: string;
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

async function isExecutable(path: string): Promise<boolean> {
  try {
    await access(path, constants.X_OK);
    return true;
  } catch {
    return false;
  }
}

async function which(command: string, env: Record<string, string | undefined>): Promise<string | undefined> {
  const pathValue = env.PATH ?? "";
  for (const dir of pathValue.split(":")) {
    if (!dir) continue;
    const candidate = join(dir, command);
    if (await isExecutable(candidate)) return candidate;
  }
  return undefined;
}

/** dumpcap 需要 cap_net_raw + cap_net_admin（或 wireshark 组、或 root）才能打开 usbmon。 */
async function probeDumpcap(
  command: string,
): Promise<Pick<ProbeCapture, "interfaces" | "privilege" | "reason" | "remediation">> {
  let interfaces: ProbeCapture["interfaces"] = [];
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
      interfaces,
      privilege: "unknown",
      reason: `dumpcap -D 执行失败：${error instanceof Error ? error.message : String(error)}`,
      remediation: `确认 ${command} 可用，然后重新运行 usb_probe。`,
    };
  }
  if (interfaces.length === 0) {
    return {
      interfaces,
      privilege: "denied",
      reason: "dumpcap 没有列出任何 usbmon 接口。",
      remediation:
        "加载 usbmon 内核模块后重试：`sudo modprobe usbmon`。" +
        "容器里通常没有 usbmon，需要在宿主机抓包后把 pcapng 交给 usb_decode。",
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
  if (privileged) return { interfaces, privilege: "ok" };
  return {
    interfaces,
    privilege: "denied",
    reason: "dumpcap 存在，但当前用户没有抓包权限（需要 CAP_NET_RAW/CAP_NET_ADMIN 或 wireshark 组成员）。",
    remediation:
      `授予能力：sudo setcap cap_net_raw,cap_net_admin+eip ${command}` +
      "；或把用户加入 wireshark 组后重新登录。完成后重试 usb_probe。",
  };
}

async function probeUsbpcap(
  command: string,
): Promise<Pick<ProbeCapture, "interfaces" | "privilege" | "reason" | "remediation">> {
  try {
    const { stdout } = await run(command, ["--extcap-interfaces"], { timeout: PROBE_TIMEOUT_MS });
    const interfaces = stdout
      .split("\n")
      .map((line) => line.trim())
      .filter((line) => line.startsWith("value") && line.includes("usbcap"))
      .map((line) => {
        const parts = line.split(",");
        const label = parts.find((part) => part.startsWith("{name=")) ?? "";
        const id = parts[parts.length - 1] ?? "";
        const bus = /usbcap(\d+)/iu.exec(id)?.[1];
        return {
          id,
          label: `${id} ${label.replace(/[{}]/gu, "")}`,
          ...(bus ? { bus: Number(bus) } : {}),
        };
      });
    if (interfaces.length > 0) return { interfaces, privilege: "unknown" };
  } catch {
    // 走兜底：USBPcapCMD 的 extcap 协议不可用时，至少给出控制设备命名约定。
  }
  return {
    interfaces: Array.from({ length: 4 }, (_, index) => ({
      id: `\\\\.\\USBPcap${index + 1}`,
      label: `\\\\.\\USBPcap${index + 1}（未能枚举，按控制设备编号约定候选）`,
      bus: index + 1,
    })),
    privilege: "unknown",
    reason: "USBPcapCMD 存在，但无法确认驱动是否已安装、当前是否以管理员身份运行。",
    remediation: "以管理员身份运行 ESCode，或先单独执行 `USBPcapCMD.exe --extcap-interfaces` 确认驱动可用。",
  };
}

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
  const configuredDumpcap = input.env.USB_PLUGIN_DUMPCAP_PATH?.trim();
  const configuredUsbpcap = input.env.USB_PLUGIN_USBPCAP_CMD_PATH?.trim();

  let capture: ProbeCapture = {
    available: false,
    backend: "none",
    interfaces: [],
    privilege: "unknown",
  };
  if (platform === "linux") {
    const command =
      (configuredDumpcap && (await isExecutable(configuredDumpcap)) ? configuredDumpcap : undefined) ??
      (await which("dumpcap", input.env));
    if (command) {
      const details = await probeDumpcap(command);
      capture = {
        available: details.interfaces.length > 0,
        backend: "dumpcap",
        command,
        interfaces: details.interfaces,
        privilege: details.privilege,
        ...(details.reason ? { reason: details.reason } : {}),
        ...(details.remediation ? { remediation: details.remediation } : {}),
      };
    } else {
      capture = {
        available: false,
        backend: "none",
        interfaces: [],
        privilege: "unknown",
        reason: "没有找到 dumpcap（Wireshark 的命令行抓包工具）。",
        remediation:
          "安装 Wireshark（Debian/Ubuntu：`sudo apt install tshark`）后授予能力：" +
          "`sudo setcap cap_net_raw,cap_net_admin+eip $(which dumpcap)`；" +
          "也可以先在别的机器上抓包，把 pcapng 交给我用 usb_decode 分析。",
      };
    }
  } else if (platform === "win32") {
    const candidates = [
      configuredUsbpcap,
      await which("USBPcapCMD.exe", input.env),
      input.env.ProgramFiles ? join(input.env.ProgramFiles, "USBPcap", "USBPcapCMD.exe") : undefined,
      input.env["ProgramFiles(x86)"]
        ? join(input.env["ProgramFiles(x86)"], "Wireshark", "USBPcapCMD.exe")
        : undefined,
    ].filter((value): value is string => Boolean(value));
    let command: string | undefined;
    for (const candidate of candidates) {
      if (await isExecutable(candidate)) {
        command = candidate;
        break;
      }
    }
    if (command) {
      const details = await probeUsbpcap(command);
      capture = {
        available: true,
        backend: "usbpcap",
        command,
        interfaces: details.interfaces,
        privilege: details.privilege,
        ...(details.reason ? { reason: details.reason } : {}),
        ...(details.remediation ? { remediation: details.remediation } : {}),
      };
    } else {
      capture = {
        available: false,
        backend: "none",
        interfaces: [],
        privilege: "unknown",
        reason: "没有找到 USBPcapCMD.exe。",
        remediation:
          "安装 Wireshark（安装时勾选 USBPcap）或单独安装 USBPcap，之后以管理员身份运行 ESCode；" +
          "也可以在别的机器上抓包后把 pcapng 交给我分析。",
      };
    }
  } else {
    capture = {
      available: false,
      backend: "none",
      interfaces: [],
      privilege: "unknown",
      reason: `${platform} 没有公开的内核级 USB 抓包接口。`,
      remediation:
        "用硬件分析仪（Lecroy / Beagle / Total Phase）或另一台 Linux/Windows 机器抓包，" +
        "把 pcapng 文件交给我用 usb_decode 分析；离线分析在本机完全可用。",
    };
  }

  if (platform === "linux" && capture.available && capture.privilege !== "ok") {
    // 能力可用但没权限：u_probe 的 available 仍为 true（后端存在、接口可枚举），
    // 由 remediation 说明为什么 usb_capture 会失败——这比直接说「不可用」更有用。
    notes.push("后端存在但权限不足：usb_capture 会返回 permission 与 remediation，不要编造抓包结果。");
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
