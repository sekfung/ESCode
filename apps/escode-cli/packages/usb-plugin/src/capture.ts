/**
 * 实时抓包（docs/specs/usb-agent-tools.md §3.3 / §5）。
 *
 * 唯一所有者是这里的抓包作业表：一次调用一个子进程，结束时一定收尾（正常退出、工具取消、进程被杀）。
 * 抓包期间写 `.partial`，正常收尾后改名——半截文件被当成完整抓包分析，比失败更糟。
 */

import { spawn, type ChildProcess } from "node:child_process";
import { mkdir, readFile, rename, stat, unlink } from "node:fs/promises";
import { dirname, isAbsolute, join, resolve } from "node:path";
import { readCaptureData } from "./pcap/pcapng.js";
import type { ProbeCapture } from "./probe.js";

export type CaptureErrorCode =
  | "unavailable"
  | "permission"
  | "busy"
  | "invalidInput"
  | "io"
  | "invalidConfig";

export class CaptureError extends Error {
  constructor(
    readonly code: CaptureErrorCode,
    message: string,
    readonly remediation?: string,
  ) {
    super(message);
    this.name = "CaptureError";
  }
}

export const CAPTURE_MAX_DURATION_MS = 60_000;
export const CAPTURE_MAX_BYTES = 512 * 1024 * 1024;

export interface CaptureRequest {
  bus?: number;
  busDev?: string;
  durationMs: number;
  maxBytes: number;
  label?: string;
  outputPath?: string;
}

export interface CaptureOutcome {
  path: string;
  bytes: number;
  packets: number;
  stoppedBy: "duration" | "maxBytes" | "signal";
  backend: string;
  target: { bus?: number; busDev?: string };
  notes: string[];
}

/** 进行中的抓包：键是「后端:总线」，同一条总线不允许两个写入者（Windows 驱动会直接拒绝）。 */
const activeCaptures = new Set<string>();

function timestampName(now: Date): string {
  const pad = (value: number, width = 2): string => String(value).padStart(width, "0");
  return (
    `${now.getUTCFullYear()}${pad(now.getUTCMonth() + 1)}${pad(now.getUTCDate())}` +
    `-${pad(now.getUTCHours())}${pad(now.getUTCMinutes())}${pad(now.getUTCSeconds())}`
  );
}

function resolveOutputPath(input: {
  request: CaptureRequest;
  workspace: string;
  pluginData: string;
}): string {
  const explicit = input.request.outputPath?.trim();
  if (explicit) {
    const target = isAbsolute(explicit) ? explicit : resolve(input.workspace, explicit);
    if (!/\.(pcapng|pcap)$/iu.test(target)) {
      throw new CaptureError("invalidInput", "outputPath 必须以 .pcapng 或 .pcap 结尾");
    }
    return target;
  }
  const label = input.request.label?.trim();
  const name = `${timestampName(new Date())}${label ? `-${label}` : ""}.pcapng`;
  return join(input.pluginData, "captures", name);
}

function pickBus(request: CaptureRequest, probe: ProbeCapture): { bus: number; notes: string[] } {
  if (request.bus !== undefined) return { bus: request.bus, notes: [] };
  if (request.busDev) {
    const bus = Number.parseInt(request.busDev.split("-")[0] ?? "", 10);
    if (Number.isFinite(bus)) {
      return { bus, notes: [`按 busDev=${request.busDev} 推导总线号 ${bus}：抓的仍是整条总线，过滤在解码期做。`] };
    }
  }
  const candidates = probe.interfaces.filter((entry) => entry.bus !== undefined);
  const only = candidates.length === 1 ? candidates[0] : undefined;
  if (only?.bus !== undefined) {
    return { bus: only.bus, notes: [`未指定总线：使用唯一的候选 ${only.label}。`] };
  }
  throw new CaptureError(
    "invalidInput",
    candidates.length === 0
      ? "无法确定要抓哪条总线：usb_probe 没有列出可用的抓包接口。"
      : `有多条抓包接口（${candidates.map((entry) => entry.label).join("；")}），请用 bus 指定。`,
  );
}

/** 子进程 stderr 到工具错误码：权限与设备名问题要给得出可执行的下一步。 */
function classifyFailure(stderr: string, command: string): CaptureError {
  const text = stderr.trim() || "抓包进程失败但没有输出";
  if (/permission denied|operation not permitted/iu.test(text)) {
    return new CaptureError(
      "permission",
      text,
      `当前用户没有抓包权限。授予能力后重试：sudo setcap cap_net_raw,cap_net_admin+eip ${command}` +
        "；或以管理员身份运行（Windows 需要管理员权限）。",
    );
  }
  if (/no such device|open failed|could not open/iu.test(text)) {
    return new CaptureError("invalidInput", text, "总线号或控制设备名可能不对：重新运行 usb_probe 核对。");
  }
  return new CaptureError("io", text);
}

function killChild(child: ChildProcess | undefined): void {
  if (!child || child.exitCode !== null || child.signalCode !== null) return;
  child.kill("SIGTERM");
  const timer = setTimeout(() => {
    if (child.exitCode === null && child.signalCode === null) child.kill("SIGKILL");
  }, 2000);
  timer.unref();
}

async function countRecords(path: string): Promise<{ packets: number; bytes: number }> {
  const data = await readFile(path);
  let packets = 0;
  try {
    readCaptureData(data, () => {
      packets += 1;
    });
  } catch {
    // 文件被中途终止的抓包工具写坏时，packets 保持 0 并由上层提示「文件可用性待确认」。
  }
  return { packets, bytes: data.byteLength };
}

export async function runCapture(input: {
  request: CaptureRequest;
  probe: ProbeCapture;
  workspace: string;
  pluginData: string;
  signal: AbortSignal;
}): Promise<CaptureOutcome> {
  const { request, probe } = input;
  if (request.durationMs > CAPTURE_MAX_DURATION_MS || request.durationMs < 1) {
    throw new CaptureError("invalidInput", `durationMs 必须在 1–${CAPTURE_MAX_DURATION_MS} 之间`);
  }
  if (request.maxBytes > CAPTURE_MAX_BYTES || request.maxBytes < 1) {
    throw new CaptureError("invalidInput", `maxBytes 必须在 1–${CAPTURE_MAX_BYTES} 之间`);
  }
  if (!probe.available || probe.backend === "none" || !probe.command) {
    throw new CaptureError(
      "unavailable",
      probe.reason ?? "本机没有可用的 USB 抓包后端。",
      probe.remediation,
    );
  }
  const { bus, notes } = pickBus(request, probe);
  const outputPath = resolveOutputPath({
    request,
    workspace: input.workspace,
    pluginData: input.pluginData,
  });
  const partialPath = `${outputPath}.partial`;
  const jobKey = `${probe.backend}:${bus}`;
  if (activeCaptures.has(jobKey)) {
    throw new CaptureError(
      "busy",
      `同一条总线（${bus}）上已经有一次抓包在进行中。`,
      "等它结束后重试；Windows 的 USBPcap 驱动同一控制设备不允许两个读者。",
    );
  }

  await mkdir(dirname(outputPath), { recursive: true });
  activeCaptures.add(jobKey);
  let child: ChildProcess | undefined;
  /** 由定时器、体积监视或取消回调写入；dumpcap 自己按 -a duration 收尾时保持未设置。 */
  let stopReason: CaptureOutcome["stoppedBy"] | undefined;
  const notesOut = [...notes];
  try {
    if (input.signal.aborted) throw new CaptureError("io", "调用在开始抓包前被取消");

    const durationSeconds = Math.max(1, Math.ceil(request.durationMs / 1000));
    const args =
      probe.backend === "dumpcap"
        ? [
            "-i",
            `usbmon${bus}`,
            "-a",
            `duration:${durationSeconds}`,
            "-a",
            `filesize:${Math.max(1, Math.ceil(request.maxBytes / 1024))}`,
            "-q",
            "-w",
            partialPath,
          ]
        : ["-d", `\\\\.\\USBPcap${bus}`, "-o", partialPath];

    child = spawn(probe.command, args, { windowsHide: true, stdio: ["ignore", "ignore", "pipe"] });
    let stderr = "";
    child.stderr?.on("data", (chunk: Buffer) => {
      stderr += chunk.toString();
      // 只保留尾部：抓包工具的告警可能很长，但真正的原因通常在最后几行。
      if (stderr.length > 8192) stderr = stderr.slice(-8192);
    });

    const timer = setTimeout(() => {
      // dumpcap 自己按 -a duration 收尾；这个兜底主要给 USBPcapCMD（没有时长参数）。
      stopReason = "duration";
      killChild(child);
    }, request.durationMs + (probe.backend === "dumpcap" ? 4000 : 200));
    const sizeMonitor =
      probe.backend === "usbpcap"
        ? setInterval(() => {
            void stat(partialPath)
              .then((info) => {
                if (info.size >= request.maxBytes) {
                  stopReason = "maxBytes";
                  killChild(child);
                }
              })
              .catch(() => undefined);
          }, 500)
        : undefined;

    const aborted = new Promise<never>((_, reject) => {
      const onAbort = (): void => {
        stopReason = "signal";
        killChild(child);
        reject(new CaptureError("io", "抓包被取消（工具调用中止或 MCP 连接断开）"));
      };
      if (input.signal.aborted) onAbort();
      else input.signal.addEventListener("abort", onAbort, { once: true });
    });

    const exitCode = await Promise.race([
      new Promise<number | null>((resolveExit) => {
        child?.once("close", (code) => resolveExit(code));
        child?.once("error", (error) => {
          stderr += error.message;
          resolveExit(-1);
        });
      }),
      aborted,
    ]).finally(() => {
      clearTimeout(timer);
      if (sizeMonitor) clearInterval(sizeMonitor);
    });

    // dumpcap 正常结束时返回 0；被我们终止（时长到、体积到）会是非 0 退出，
    // 这属于预期路径：只要文件里已经有可用数据就照常交付，而不是报失败。
    const partialInfo = await stat(partialPath).catch(() => undefined);
    const hasData = (partialInfo?.size ?? 0) > 24;
    if ((exitCode ?? 0) !== 0 && !hasData && stopReason !== "signal") {
      await unlink(partialPath).catch(() => undefined);
      throw classifyFailure(stderr, probe.command);
    }
    if (stderr.trim()) notesOut.push(`抓包工具输出：${stderr.trim().split("\n").slice(-3).join(" | ")}`);
    await rename(partialPath, outputPath);
    const counted = await countRecords(outputPath);
    if (counted.packets === 0) {
      notesOut.push(
        "抓包里没有任何包：确认总线号是否正确，以及抓包期间设备上确实有流量" +
          "（可以请用户在这段时间里重新插拔设备或复现操作）。",
      );
    }
    return {
      path: outputPath,
      bytes: counted.bytes,
      packets: counted.packets,
      stoppedBy: stopReason ?? "duration",
      backend: probe.backend,
      target: { bus, ...(request.busDev ? { busDev: request.busDev } : {}) },
      notes: notesOut,
    };
  } catch (error) {
    killChild(child);
    await unlink(partialPath).catch(() => undefined);
    throw error;
  } finally {
    activeCaptures.delete(jobKey);
  }
}
