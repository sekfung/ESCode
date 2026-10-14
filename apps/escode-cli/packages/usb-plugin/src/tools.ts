/**
 * USB 工具定义与调用处理（docs/specs/usb-agent-tools.md §3）。
 *
 * 三个工具构成单向漏斗「探 → 抓 → 解」，职责不重叠；description 里写清**什么时候该用它**，
 * 因为模型选错工具多半是描述里只写了能力、没写触发条件。
 */

import { readFile, stat } from "node:fs/promises";
import { isAbsolute, resolve } from "node:path";
import { z } from "zod";
import { analyzeCapture } from "./analysis.js";
import { CaptureError, runCapture } from "./capture.js";
import { CaptureFormatError } from "./pcap/pcapng.js";
import { probeEnvironment } from "./probe.js";

export interface UsbToolDefinition {
  name: string;
  description: string;
  inputSchema: { type: "object"; [key: string]: unknown };
  annotations: { readOnlyHint?: boolean; destructiveHint?: boolean; title: string };
}

export interface UsbToolResult {
  content: Array<{ type: "text"; text: string }>;
  isError?: true;
}

export interface UsbToolContext {
  env: Record<string, string | undefined>;
  workspace: string;
  pluginData: string;
  signal: AbortSignal;
}

export const USB_TOOL_NAMES = ["usb_probe", "usb_capture", "usb_decode"] as const;
export type UsbToolName = (typeof USB_TOOL_NAMES)[number];
/** 免审批清单，需与官方插件定义的 `defaultAllowedTools` 一致（有测试钉住）。 */
export const USB_READ_ONLY_TOOLS: readonly UsbToolName[] = ["usb_probe", "usb_decode"];

const DEFAULT_DURATION_MS = 10_000;
const DEFAULT_MAX_BYTES = 64 * 1024 * 1024;
/** 解码上限：再大就该先用抓包工具裁剪，而不是把整个文件读进内存。 */
const MAX_DECODE_BYTES = 1024 * 1024 * 1024;

const probeSchema = z.object({}).strict();
const captureSchema = z
  .object({
    bus: z.number().int().min(1).max(64).optional(),
    busDev: z
      .string()
      .regex(/^\d+-\d+$/u, "busDev 形如 1-4（来自 usb_probe 的设备列表）")
      .optional(),
    durationMs: z.number().int().min(1).max(60_000).default(DEFAULT_DURATION_MS),
    maxBytes: z.number().int().min(1).max(512 * 1024 * 1024).default(DEFAULT_MAX_BYTES),
    label: z
      .string()
      .regex(/^[a-z0-9-]{1,32}$/u, "label 只允许小写字母、数字与连字符")
      .optional(),
    outputPath: z.string().trim().min(1).max(512).optional(),
  })
  .strict();
const decodeSchema = z
  .object({
    source: z.string().trim().min(1).max(4096),
    protocol: z.enum(["auto", "ccid", "hid", "msc", "raw"]).default("auto"),
    view: z.enum(["summary", "timeline", "message"]).default("summary"),
    filter: z
      .object({
        busDev: z.string().trim().min(1).max(64).optional(),
        endpoint: z.number().int().min(0).max(255).optional(),
        direction: z.enum(["in", "out"]).optional(),
        onlyErrors: z.boolean().optional(),
      })
      .strict()
      .optional(),
    index: z.number().int().min(1).optional(),
    limit: z.number().int().min(1).max(500).default(200),
    cursor: z.string().trim().min(1).max(32).optional(),
    payloadBytes: z.number().int().min(1).max(4096).default(256),
  })
  .strict();

const DESCRIPTIONS: Record<UsbToolName, string> = {
  usb_probe:
    "First step for any USB problem on this machine: report the capture backend (dumpcap on Linux, USBPcapCMD on Windows) " +
    "with its privilege state and an exact remediation command when it is not usable, the attached USB devices with their " +
    "interface classes (this is what tells you CCID vs HID vs mass storage), and existing capture files. " +
    "Call it before usb_capture; when it reports available=false do not attempt usb_capture — pass its remediation to the user instead. " +
    "Read-only, no approval.",
  usb_capture:
    "Record a bounded slice of USB traffic into a .pcapng file for usb_decode. Capture ships with ESCode itself " +
    "(built-in backend: a small helper on Linux, a capture service on Windows); a user-installed dumpcap is only a fallback. " +
    "Use it when the failure must be reproduced live and no capture file exists yet. " +
    "This needs the user's approval: explain what you will capture and ask the user to reproduce the problem (or replug the device) " +
    "during the capture window. Capture is per bus; narrow to one device with busDev at decode time. " +
    "If it fails with permission/unavailable, relay usb_probe's remediation (on Linux the first capture needs a one-time " +
    "system authorization the user must approve) instead of retrying.",
  usb_decode:
    "Decode a .pcapng/.pcap USB capture into protocol-level messages: CCID card-reader messages with APDU and SW1SW2, " +
    "HID reports decoded against the report descriptor, MSC/SCSI commands with CBW/CDB/CSW and sense data. " +
    "Offline and read-only: no privileges, works on every platform (including macOS) and with files captured by Wireshark, " +
    "USBPcap, usbmon or a hardware analyzer. Flow: view=summary first (devices, protocol spread, anomalies), " +
    "then view=timeline with cursor paging, then view=message with index for one message's bytes. " +
    "Pick protocol from the device's interface class; use protocol=auto when unsure, protocol=raw to inspect traffic " +
    "without protocol decoding. Report findings in protocol terms (APDU/SW, CDB/CSW/sense, HID fields), not raw hex.",
};

const schemas: Record<UsbToolName, z.ZodType> = {
  usb_probe: probeSchema,
  usb_capture: captureSchema,
  usb_decode: decodeSchema,
};

export const USB_TOOLS: UsbToolDefinition[] = USB_TOOL_NAMES.map((name) => ({
  name,
  description: DESCRIPTIONS[name],
  inputSchema: z.toJSONSchema(schemas[name], { io: "input" }) as { type: "object" },
  annotations: USB_READ_ONLY_TOOLS.includes(name)
    ? { title: name, readOnlyHint: true }
    : { title: name, destructiveHint: true },
}));

/** 工具失败：code 与 serial 工具同一套命名，便于模型跨插件复用经验。 */
export class ToolFailure extends Error {
  constructor(
    readonly code: string,
    message: string,
    readonly remediation?: string,
  ) {
    super(message);
    this.name = "ToolFailure";
  }
}

function failure(code: string, message: string, remediation?: string): UsbToolResult {
  const suffix = remediation ? `\n下一步：${remediation}` : "";
  return { content: [{ type: "text", text: `[${code}] ${message}${suffix}` }], isError: true };
}

function success(text: string, meta?: Record<string, unknown>): UsbToolResult {
  return {
    content: [
      { type: "text", text },
      ...(meta ? [{ type: "text" as const, text: JSON.stringify(meta) }] : []),
    ],
  };
}

/** `source` 可以是工作区相对路径，也可以是用户给的任意绝对路径（读的是用户自己的文件）。 */
async function resolveSourcePath(source: string, workspace: string): Promise<string> {
  const path = isAbsolute(source) ? source : resolve(workspace, source);
  const info = await stat(path).catch(() => undefined);
  if (!info?.isFile()) throw new ToolFailure("notFound", `找不到抓包文件：${path}`);
  if (info.size > MAX_DECODE_BYTES) {
    throw new ToolFailure(
      "invalidInput",
      `抓包 ${(info.size / 1024 / 1024).toFixed(0)} MiB，超过单次解码上限（1 GiB）：` +
        "先用 Wireshark 裁剪时间范围或按设备导出，再交给我分析。",
    );
  }
  return path;
}

async function runProbe(context: UsbToolContext): Promise<UsbToolResult> {
  const result = await probeEnvironment({
    env: context.env,
    workspace: context.workspace,
    pluginData: context.pluginData,
  });
  const lines = [
    `平台：${result.platform}`,
    result.capture.available
      ? `抓包：可用（${result.capture.backend}：${result.capture.command}，权限 ${result.capture.privilege}）`
      : `抓包：不可用（${result.capture.reason ?? "没有后端"}）`,
    ...(result.capture.interfaces.length > 0
      ? [`抓包接口：${result.capture.interfaces.map((entry) => entry.label).join("；")}`]
      : []),
    ...(result.capture.remediation ? [`修复方式：${result.capture.remediation}`] : []),
    // 每个不可用后端都要说清原因：用户最常问的就是「我装了 Wireshark 为什么还不行」，
    // 而「首选后端不可用」这句话本身不含信息。
    ...result.capture.backends
      .filter((backend) => !backend.available)
      .map((backend) => `抓包后端 ${backend.id} 不可用：${backend.reason ?? "原因未知"}`),
    `设备（${result.devices.length}）：`,
    ...result.devices.map((device) => {
      const ids = [device.vid, device.pid].filter(Boolean).join(":");
      const interfaces = device.interfaces
        .map(
          (entry) =>
            `${entry.className}${entry.protocols.length > 0 ? `(${entry.protocols.join("/")})` : ""}`,
        )
        .join("、");
      return (
        `  ${device.busDev ?? "（无总线地址）"}  ${ids || "无 ID"}  ${device.product ?? "未知设备"}` +
        `${interfaces ? `  接口：${interfaces}` : ""}${device.addressNote ? `  ${device.addressNote}` : ""}`
      );
    }),
    ...(result.captures.length > 0
      ? [
          `已有抓包（${result.captures.length}）：`,
          ...result.captures.map(
            (entry) => `  ${entry.path}  ${(entry.bytes / 1024).toFixed(0)} KiB  ${entry.modifiedAt}`,
          ),
        ]
      : ["已有抓包：无"]),
    ...(result.notes.length > 0 ? ["提示：", ...result.notes.map((note) => `  ${note}`)] : []),
  ];
  return success(lines.join("\n"), {
    platform: result.platform,
    captureAvailable: result.capture.available,
    devices: result.devices.length,
    captures: result.captures.length,
  });
}

async function runCaptureTool(
  context: UsbToolContext,
  request: z.infer<typeof captureSchema>,
): Promise<UsbToolResult> {
  const probe = await probeEnvironment({
    env: context.env,
    workspace: context.workspace,
    pluginData: context.pluginData,
  });
  const outcome = await runCapture({
    request,
    probe: probe.capture,
    workspace: context.workspace,
    pluginData: context.pluginData,
    signal: context.signal,
    env: context.env,
  });
  return success(
    [
      `已抓包：${outcome.path}`,
      `${outcome.packets} 个包，${(outcome.bytes / 1024).toFixed(0)} KiB，结束原因：${outcome.stoppedBy}`,
      `后端：${outcome.backend}；目标：总线 ${outcome.target.bus ?? "?"}${outcome.target.busDev ? `（解码时只看 ${outcome.target.busDev}）` : ""}`,
      ...outcome.notes.map((note) => `提示：${note}`),
      "下一步：usb_decode 传这个路径，先 view=summary 看设备与协议分布。",
    ].join("\n"),
    {
      path: outcome.path,
      packets: outcome.packets,
      bytes: outcome.bytes,
      stoppedBy: outcome.stoppedBy,
    },
  );
}

async function runDecode(
  context: UsbToolContext,
  request: z.infer<typeof decodeSchema>,
): Promise<UsbToolResult> {
  const path = await resolveSourcePath(request.source, context.workspace);
  const data = await readFile(path);
  const analysis = analyzeCapture(new Uint8Array(data), {
    protocol: request.protocol,
    view: request.view,
    ...(request.filter ? { filter: request.filter } : {}),
    ...(request.index !== undefined ? { index: request.index } : {}),
    limit: request.limit,
    ...(request.cursor ? { cursor: request.cursor } : {}),
    payloadBytes: request.payloadBytes,
  });
  return success(`${analysis.text}\n文件：${path}`, { ...analysis.meta, path });
}

export async function handleUsbToolCall(input: {
  name: string;
  args: unknown;
  context: UsbToolContext;
}): Promise<UsbToolResult> {
  const name = input.name as UsbToolName;
  try {
    switch (name) {
      case "usb_probe": {
        const parsed = probeSchema.safeParse(input.args ?? {});
        if (!parsed.success) return failure("invalidInput", parsed.error.message);
        return await runProbe(input.context);
      }
      case "usb_capture": {
        const parsed = captureSchema.safeParse(input.args ?? {});
        if (!parsed.success) return failure("invalidInput", parsed.error.message);
        return await runCaptureTool(input.context, parsed.data);
      }
      case "usb_decode": {
        const parsed = decodeSchema.safeParse(input.args ?? {});
        if (!parsed.success) return failure("invalidInput", parsed.error.message);
        return await runDecode(input.context, parsed.data);
      }
      default:
        return failure("invalidInput", `Unknown tool: ${input.name}`);
    }
  } catch (error) {
    if (error instanceof ToolFailure) return failure(error.code, error.message, error.remediation);
    if (error instanceof CaptureError) return failure(error.code, error.message, error.remediation);
    if (error instanceof CaptureFormatError) {
      return failure(
        error.code === "unsupportedFormat" ? "unsupportedFormat" : "decode",
        error.message,
      );
    }
    return failure("io", error instanceof Error ? error.message : String(error));
  }
}
