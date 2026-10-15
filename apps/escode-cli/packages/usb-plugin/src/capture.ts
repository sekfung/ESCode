/**
 * 实时抓包（docs/specs/usb-agent-tools.md §3.3 / §5）。
 *
 * 只有一个后端：Windows 上的 `escode-usb-capture` 服务。插件是**作业所有者**——它负责参数校验、
 * 超时、取消与把结果翻译成工具结果；抓包器只接受固定参数、写一个 pcapng 文件、回报一行 JSON。
 * Linux 与 macOS 没有实时抓包，只有离线解码（`backends.ts` 的注释里写了为什么不做 dumpcap 回退）。
 */

import { spawn, type ChildProcess } from "node:child_process";
import { mkdir, readFile, stat } from "node:fs/promises";
import { dirname, isAbsolute, join, resolve } from "node:path";
import { selectCaptureBackend } from "./backends.js";
import { readCaptureData } from "./pcap/pcapng.js";
import { offlineOnlyGuidance, type ProbeCapture } from "./probe.js";

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
  backend: "builtin";
  target: { bus?: number; busDev?: string };
  notes: string[];
}

/** 进行中的抓包：键是「后端:总线」，同一条总线不允许两个写入者（Windows 驱动会直接拒绝第二个）。 */
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

function pickBus(
  request: CaptureRequest,
  offline: Pick<ProbeCapture, "interfaces">,
): { bus: number; notes: string[] } {
  if (request.bus !== undefined) return { bus: request.bus, notes: [] };
  if (request.busDev) {
    const bus = Number.parseInt(request.busDev.split("-")[0] ?? "", 10);
    if (Number.isFinite(bus)) {
      return { bus, notes: [`按 busDev=${request.busDev} 推导总线号 ${bus}：抓的仍是整条总线，过滤在解码期做。`] };
    }
  }
  const candidates = offline.interfaces.filter((entry) => entry.bus !== undefined);
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
    // 文件被中途终止的抓包器写坏时，packets 保持 0 并由上层提示「文件可用性待确认」。
  }
  return { packets, bytes: data.byteLength };
}

/** 自研抓包器（builtin 后端）：它自己写 `.partial` 并在正常收尾时改名成最终文件。 */
async function runBuiltinCapture(input: {
  helper: string;
  request: CaptureRequest;
  bus: number;
  busDev?: string;
  outputPath: string;
  signal: AbortSignal;
  notes: string[];
  /** 显式传插件自己的 env：不传会继承宿主进程环境，行为随启动方式漂移（测试也无法注入）。 */
  env: Record<string, string | undefined>;
}): Promise<CaptureOutcome> {
  const args = [
    "--bus",
    String(input.bus),
    "--duration-ms",
    String(input.request.durationMs),
    "--max-bytes",
    String(input.request.maxBytes),
    "--output",
    input.outputPath,
  ];
  const child = spawn(input.helper, args, {
    env: input.env,
    windowsHide: true,
    stdio: ["ignore", "pipe", "pipe"],
  });
  let stdout = "";
  let stderr = "";
  child.stdout?.on("data", (chunk: Buffer) => {
    stdout += chunk.toString();
  });
  child.stderr?.on("data", (chunk: Buffer) => {
    stderr += chunk.toString();
    if (stderr.length > 8192) stderr = stderr.slice(-8192);
  });

  const aborted = new Promise<never>((_, reject) => {
    const onAbort = (): void => {
      killChild(child);
      reject(new CaptureError("io", "抓包被取消（工具调用中止或 MCP 连接断开）"));
    };
    if (input.signal.aborted) onAbort();
    else input.signal.addEventListener("abort", onAbort, { once: true });
  });
  const timeout = setTimeout(() => {
    // 抓包器自己按 duration-ms 收尾；这里只是兜底，防止它卡住不退出。
    killChild(child);
  }, input.request.durationMs + 5000);

  const exitCode = await Promise.race([
    new Promise<number | null>((resolveExit) => {
      child.once("close", (code) => resolveExit(code));
      child.once("error", (error) => {
        stderr += error.message;
        resolveExit(-1);
      });
    }),
    aborted,
  ]).finally(() => clearTimeout(timeout));

  // 契约是「恒为一行 JSON」，取最后一行：途中可能有诊断输出。
  const line = stdout.trim().split("\n").filter(Boolean).pop();
  let payload: Record<string, unknown> | undefined;
  if (line) {
    try {
      payload = JSON.parse(line) as Record<string, unknown>;
    } catch {
      payload = undefined;
    }
  }
  const notes = [...input.notes];
  if (payload?.ok === true) {
    if (typeof payload.dropped === "number" && payload.dropped > 0) {
      notes.push(`抓包器报告丢弃 ${payload.dropped} 个事件（驱动缓冲溢出）：结论要按「可能缺报文」来读。`);
    }
    if (typeof payload.skippedIsochronous === "number" && payload.skippedIsochronous > 0) {
      notes.push(`跳过 ${payload.skippedIsochronous} 个等时传输事件（不在解码范围内）。`);
    }
    const stoppedBy =
      payload.stoppedBy === "maxBytes" || payload.stoppedBy === "signal"
        ? payload.stoppedBy
        : "duration";
    // 抓包器收到信号时会**优雅收尾**并如实返回 ok:true，但产物是 `<output>.partial`（它不假装
    // 半截文件是完整产物）。外部信号（不是我们的 abort）走的就是这条路，所以这里要按它自报的
    // 路径收尾，否则会去读一个不存在的最终文件、把「有部分数据可分析」变成一句 ENOENT。
    const reported = typeof payload.output === "string" ? payload.output : undefined;
    const finalExists = await stat(input.outputPath)
      .then(() => true)
      .catch(() => false);
    const effectivePath = finalExists ? input.outputPath : (reported ?? input.outputPath);
    if (!finalExists) {
      const exists = await stat(effectivePath)
        .then(() => true)
        .catch(() => false);
      if (!exists) {
        throw new CaptureError(
          "io",
          `抓包器报告成功但产物不存在：${input.outputPath}（抓包器自报路径 ${reported ?? "无"}）。`,
        );
      }
      notes.push(
        `抓包被中断，产物是部分文件：${effectivePath}（可先用 usb_decode 分析已抓到的部分）。`,
      );
    }
    const counted = await countRecords(effectivePath);
    return {
      path: effectivePath,
      bytes: typeof payload.bytes === "number" ? payload.bytes : counted.bytes,
      packets: typeof payload.packets === "number" ? payload.packets : counted.packets,
      stoppedBy,
      backend: "builtin",
      target: { bus: input.bus, ...(input.busDev ? { busDev: input.busDev } : {}) },
      notes,
    };
  }

  // 失败：优先用抓包器自报的错误码与 remediation（它比我们更清楚缺哪一步）。
  const error = (payload?.error ?? undefined) as Record<string, unknown> | undefined;
  const code =
    error?.code === "permission" || error?.code === "busy"
      ? error.code
      : error?.code === "invalidInput" || error?.code === "no-interface"
        ? "invalidInput"
        : "io";
  const partialNote = await stat(`${input.outputPath}.partial`)
    .then(() => `已保留部分文件：${input.outputPath}.partial（可先用 usb_decode 分析已抓到的部分）。`)
    .catch(() => undefined);
  throw new CaptureError(
    code,
    typeof error?.message === "string"
      ? error.message
      : `抓包器失败（退出码 ${exitCode ?? "?"}）：${(stderr.trim() || "无输出").split("\n").slice(-2).join(" | ")}`,
    [typeof error?.remediation === "string" ? error.remediation : undefined, partialNote]
      .filter(Boolean)
      .join("\n") || undefined,
  );
}

export async function runCapture(input: {
  request: CaptureRequest;
  probe: ProbeCapture;
  workspace: string;
  pluginData: string;
  signal: AbortSignal;
  env: Record<string, string | undefined>;
}): Promise<CaptureOutcome> {
  const { request, probe } = input;
  if (request.durationMs > CAPTURE_MAX_DURATION_MS || request.durationMs < 1) {
    throw new CaptureError("invalidInput", `durationMs 必须在 1–${CAPTURE_MAX_DURATION_MS} 之间`);
  }
  if (request.maxBytes > CAPTURE_MAX_BYTES || request.maxBytes < 1) {
    throw new CaptureError("invalidInput", `maxBytes 必须在 1–${CAPTURE_MAX_BYTES} 之间`);
  }
  const selection = selectCaptureBackend({
    ...(probe.preferred ? { preferred: probe.preferred } : {}),
    backends: probe.backends,
  });
  if (!selection.backend) {
    const failure = selection.failure;
    throw new CaptureError(
      "unavailable",
      failure?.reason ?? probe.reason ?? offlineOnlyGuidance().reason,
      failure?.remediation ?? probe.remediation ?? offlineOnlyGuidance().remediation,
    );
  }
  const activeBackend = selection.backend;
  if (!activeBackend.command) {
    throw new CaptureError(
      "unavailable",
      "抓包后端已登记但缺少可执行文件路径（探测结果不完整）。",
      offlineOnlyGuidance().remediation,
    );
  }
  const { bus, notes } = pickBus(request, { interfaces: activeBackend.interfaces });
  const outputPath = resolveOutputPath({
    request,
    workspace: input.workspace,
    pluginData: input.pluginData,
  });
  const jobKey = `${activeBackend.id}:${bus}`;
  if (activeCaptures.has(jobKey)) {
    throw new CaptureError(
      "busy",
      `同一条总线（${bus}）上已经有一次抓包在进行中。`,
      "等它结束后重试；不同总线可以并发抓包。",
    );
  }

  await mkdir(dirname(outputPath), { recursive: true });
  activeCaptures.add(jobKey);
  try {
    if (input.signal.aborted) throw new CaptureError("io", "调用在开始抓包前被取消");
    return await runBuiltinCapture({
      helper: activeBackend.command,
      request,
      bus,
      ...(request.busDev ? { busDev: request.busDev } : {}),
      outputPath,
      signal: input.signal,
      notes,
      env: input.env,
    });
  } finally {
    activeCaptures.delete(jobKey);
  }
}
