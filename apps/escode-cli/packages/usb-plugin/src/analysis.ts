/**
 * 分析编排与视图（docs/specs/usb-agent-tools.md §3.4 / §8）。
 *
 * 链路：容器 → 链路层 → 逻辑报文 → 按设备分组 → 协议消息 → 视图裁剪。
 * 所有视图都有硬上限：summary 恒定小体积，timeline 分页，message 单条——抓包动辄数万条，
 * 任何「把结果全量交给模型」的实现都会把上下文烧光。
 */

import { decodeCcidMessages } from "./decode/ccid.js";
import { decodeHidMessages } from "./decode/hid.js";
import { decodeMscMessages } from "./decode/msc.js";
import { toHex } from "./format.js";
import { classifyDevices } from "./pcap/classify.js";
import { CaptureFormatError, readCaptureData, type CaptureInterface } from "./pcap/pcapng.js";
import { toExchanges } from "./pcap/reassemble.js";
import { decodeUsbpcapRecord, isUsbpcapLinkType } from "./pcap/usbpcap.js";
import { decodeUsbmonRecord, isUsbmonLinkType, type LinkProblem } from "./pcap/usbmon.js";
import {
  type DecodedMessage,
  type DecodeContext,
  type DeviceSummary,
  type ProtocolName,
  type UsbDirection,
  type UsbExchange,
  type UsbTransfer,
  interfaceClassName,
} from "./types.js";

export type AnalysisView = "summary" | "timeline" | "message";

export interface AnalysisFilter {
  busDev?: string;
  endpoint?: number;
  direction?: UsbDirection;
  onlyErrors?: boolean;
}

export interface AnalysisOptions {
  protocol: ProtocolName | "auto";
  view: AnalysisView;
  filter?: AnalysisFilter;
  index?: number;
  limit?: number;
  cursor?: string;
  payloadBytes?: number;
}

export interface AnalysisOutput {
  /** 给模型看的紧凑文本（一行一条报文）。 */
  text: string;
  /** 结构化元数据：计数、游标、截断标志；体积固定，不含逐条报文。 */
  meta: Record<string, unknown>;
}

const DEFAULT_TIMELINE_LIMIT = 200;
const MAX_TIMELINE_LIMIT = 500;
const DEFAULT_PAYLOAD_BYTES = 256;
const MAX_PAYLOAD_BYTES = 4096;
const MAX_ANOMALIES_LISTED = 50;
const MAX_NOTES = 12;
/** 单次分析的包数上限：超过就截断分析并明确告知，而不是把内存吃满。 */
const MAX_RECORDS = 500_000;

interface PreparedCapture {
  format: string;
  interfaces: CaptureInterface[];
  records: number;
  truncated: boolean;
  transfers: UsbTransfer[];
  problems: string[];
  notes: string[];
}

function decodeRecords(data: Uint8Array): PreparedCapture {
  const transfers: UsbTransfer[] = [];
  const problems: LinkProblem[] = [];
  const notes: string[] = [];
  const skippedLinkTypes = new Set<number>();
  let records = 0;
  const result = readCaptureData(data, (record) => {
    if (records >= MAX_RECORDS) return;
    records += 1;
    const linkType = record.linkType;
    if (isUsbmonLinkType(linkType)) {
      const transfer = decodeUsbmonRecord(linkType, record.data, record.tsMicros, problems);
      if (transfer) transfers.push(transfer);
    } else if (isUsbpcapLinkType(linkType)) {
      const transfer = decodeUsbpcapRecord(record.data, record.tsMicros, problems);
      if (transfer) transfers.push(transfer);
    } else {
      skippedLinkTypes.add(linkType);
    }
  });
  // 「没有 USB 链路」是文件本身的问题：要在分析任何包之前就说清楚，而不是给出一份空结果。
  const usbInterfaces = result.interfaces.filter(
    (entry) => isUsbmonLinkType(entry.linkType) || isUsbpcapLinkType(entry.linkType),
  );
  if (usbInterfaces.length === 0) {
    throw new CaptureFormatError(
      "unsupportedFormat",
      `链路类型 ${result.interfaces.map((entry) => entry.linkType).join("/")} 不是 USB 抓包；` +
        "支持 189/220（Linux usbmon）与 249（USBPcap）",
    );
  }
  if (skippedLinkTypes.size > 0) {
    notes.push(`跳过了非 USB 链路的包（链路类型 ${[...skippedLinkTypes].join("/")}）。`);
  }
  if (result.truncated) notes.push("文件被截断（抓包未正常收尾）：只分析已解析的部分。");
  if (result.records > MAX_RECORDS) {
    notes.push(`抓包共 ${result.records} 个包，只分析了前 ${MAX_RECORDS} 个。`);
  }
  for (const problem of problems.slice(0, MAX_NOTES)) notes.push(problem.message);
  if (problems.length > MAX_NOTES) notes.push(`另有 ${problems.length - MAX_NOTES} 条解析告警未列出。`);
  return {
    format: result.format,
    interfaces: result.interfaces,
    records,
    truncated: result.truncated,
    transfers,
    problems: problems.map((problem) => problem.message),
    notes,
  };
}

function requestName(setup: UsbExchange["setup"]): string | undefined {
  if (!setup) return undefined;
  const standard = (setup.bmRequestType & 0x60) === 0;
  if (!standard) return undefined;
  const names: Record<number, string> = {
    0x00: "GET_STATUS",
    0x01: "CLEAR_FEATURE",
    0x03: "SET_FEATURE",
    0x05: "SET_ADDRESS",
    0x06: "GET_DESCRIPTOR",
    0x07: "SET_DESCRIPTOR",
    0x08: "GET_CONFIGURATION",
    0x09: "SET_CONFIGURATION",
    0x0a: "GET_INTERFACE",
    0x0b: "SET_INTERFACE",
  };
  return names[setup.bRequest];
}

/** `protocol: "raw"` 的兜底视图：不做协议解码，只把逻辑报文摊开。 */
function rawMessages(exchanges: UsbExchange[]): DecodedMessage[] {
  return exchanges.map((exchange) => {
    const length = exchange.data.length;
    const anomaly =
      exchange.status !== 0
        ? `状态码 ${exchange.status}`
        : exchange.expectedLength !== undefined &&
            exchange.actualLength !== undefined &&
            exchange.expectedLength !== exchange.actualLength
          ? `短传输：请求 ${exchange.expectedLength} 字节，实际 ${exchange.actualLength} 字节`
          : undefined;
    const name = requestName(exchange.setup);
    return {
      exchanges: [exchange.index],
      ts: exchange.ts,
      busDev: exchange.busDev,
      direction: exchange.direction,
      kind: `${exchange.type === "control" ? "Control" : exchange.type === "bulk" ? "Bulk" : exchange.type === "interrupt" ? "Interrupt" : "Isochronous"}`,
      // 异常只放进 `anomalies`，由视图层统一渲染一次 ⚠；写进 summary 会和它重复。
      summary:
        `${exchange.direction === "in" ? "IN " : "OUT "}ep0x${exchange.endpoint.toString(16).padStart(2, "0")} ` +
        `${length} 字节${name ? ` ${name}` : ""}`,
      detail: [
        `传输类型：${exchange.type}，端点 0x${exchange.endpoint.toString(16).padStart(2, "0")}（${exchange.direction}）`,
        ...(exchange.setup
          ? [
              `SETUP：bmRequestType=0x${exchange.setup.bmRequestType.toString(16).padStart(2, "0")} ` +
                `bRequest=0x${exchange.setup.bRequest.toString(16).padStart(2, "0")} ` +
                `wValue=0x${exchange.setup.wValue.toString(16)} wIndex=0x${exchange.setup.wIndex.toString(16)} ` +
                `wLength=${exchange.setup.wLength}`,
            ]
          : []),
        `状态码：${exchange.status}`,
        ...(exchange.expectedLength !== undefined ? [`声明长度：${exchange.expectedLength}`] : []),
        ...(exchange.actualLength !== undefined ? [`实际长度：${exchange.actualLength}`] : []),
      ],
      anomalies: anomaly ? [anomaly] : [],
      payload: exchange.data,
    };
  });
}

const DECODERS: Record<Exclude<ProtocolName, "raw">, (context: DecodeContext) => DecodedMessage[]> = {
  ccid: decodeCcidMessages,
  hid: decodeHidMessages,
  msc: decodeMscMessages,
};

function decodeMessages(
  protocol: ProtocolName | "auto",
  devices: DeviceSummary[],
  exchanges: UsbExchange[],
): { messages: DecodedMessage[]; notes: string[] } {
  const notes: string[] = [];
  const byDevice = new Map<string, UsbExchange[]>();
  for (const exchange of exchanges) {
    const list = byDevice.get(exchange.busDev);
    if (list) list.push(exchange);
    else byDevice.set(exchange.busDev, [exchange]);
  }
  const messages: DecodedMessage[] = [];
  const requested = protocol === "auto" ? undefined : protocol;
  for (const device of devices) {
    const deviceExchanges = byDevice.get(device.busDev) ?? [];
    if (protocol === "raw") {
      messages.push(...rawMessages(deviceExchanges));
      continue;
    }
    const guessed = new Set(device.interfaces.flatMap((entry) => entry.protocols));
    if (requested && requested !== "raw" && guessed.size > 0 && !guessed.has(requested)) {
      notes.push(
        `设备 ${device.busDev} 的接口类别是 ${device.interfaces
          .map((entry) => interfaceClassName(entry.classCode))
          .join("/")}，与请求的 ${requested} 不一致：结果可能为空。`,
      );
    }
    // auto 时三套解码器都跑：每套都自带「不是我的流量就返回空」的判定，合并比猜类别更稳。
    const decoders = requested && requested !== "raw" ? [DECODERS[requested]] : Object.values(DECODERS);
    for (const decoder of decoders) {
      messages.push(...decoder({ exchanges: deviceExchanges, device }));
    }
  }
  messages.sort((left, right) => {
    const byTime = left.ts - right.ts;
    if (byTime !== 0) return byTime;
    return (left.exchanges[0] ?? 0) - (right.exchanges[0] ?? 0);
  });
  return { messages, notes };
}

/** 逻辑报文编号 → 端点：`message` 只携带归属编号，端点过滤要回到报文里查。 */
function endpointByExchangeIndex(exchanges: UsbExchange[]): Map<number, number> {
  return new Map(exchanges.map((exchange) => [exchange.index, exchange.endpoint]));
}

function applyFilter(
  messages: DecodedMessage[],
  filter: AnalysisFilter | undefined,
  endpoints: Map<number, number>,
): DecodedMessage[] {
  if (!filter) return messages;
  return messages.filter((message) => {
    if (filter.busDev && message.busDev !== filter.busDev) return false;
    if (filter.direction && message.direction !== filter.direction) return false;
    if (filter.onlyErrors && message.anomalies.length === 0) return false;
    if (filter.endpoint !== undefined) {
      const matched = message.exchanges.some(
        (index) => endpoints.get(index) === filter.endpoint,
      );
      if (!matched) return false;
    }
    return true;
  });
}

function formatDelta(micros: number): string {
  return `+${(micros / 1_000_000).toFixed(3)}s`;
}

function deviceLine(device: DeviceSummary): string {
  const ids = [
    device.videoId !== undefined ? device.videoId.toString(16).padStart(4, "0") : undefined,
    device.productId !== undefined ? device.productId.toString(16).padStart(4, "0") : undefined,
  ];
  const idText = ids[0] && ids[1] ? `${ids[0]}:${ids[1]}` : "无描述符";
  const name = [device.manufacturer, device.product].filter(Boolean).join(" ");
  const interfaces = device.interfaces.length
    ? device.interfaces
        .map((entry) => {
          const protocols = entry.protocols.length > 0 ? ` → ${entry.protocols.join("/")}` : "";
          return `接口 ${entry.number}: ${interfaceClassName(entry.classCode)}${protocols}`;
        })
        .join("；")
    : "接口未知";
  return `  ${device.busDev}  ${idText}  ${name || "未知设备"}  ${interfaces}`;
}

/** 头部行里带上接口的链路类型与接口名：usbmon / USBPcap 的线索都在这里。 */
function interfaceLine(entry: CaptureInterface): string {
  return `${entry.linkType}${entry.name ? `/${entry.name}` : ""}`;
}

export function analyzeCapture(data: Uint8Array, options: AnalysisOptions): AnalysisOutput {
  const prepared = decodeRecords(data);
  const { exchanges, warnings } = toExchanges(prepared.transfers);
  const notes = [...prepared.notes, ...warnings];
  const { devices, notes: classifyNotes } = classifyDevices(
    exchanges,
    collectAddresses(exchanges).values(),
  );
  const deviceList = [...devices.values()].sort((left, right) => left.busDev.localeCompare(right.busDev));
  const { messages, notes: decodeNotes } = decodeMessages(options.protocol, deviceList, exchanges);
  notes.push(...classifyNotes, ...decodeNotes);

  const filtered = applyFilter(messages, options.filter, endpointByExchangeIndex(exchanges));
  const first = filtered[0];
  const last = filtered[filtered.length - 1];
  const anomalyCount = filtered.reduce((total, message) => total + message.anomalies.length, 0);

  if (options.view === "summary") {
    const counts = new Map<string, number>();
    for (const message of filtered) counts.set(message.kind, (counts.get(message.kind) ?? 0) + 1);
    const lines: string[] = [];
    lines.push(
      `抓包：${prepared.format}，链路类型 ${prepared.interfaces.map(interfaceLine).join("、")}，` +
        `${prepared.records} 个包${prepared.truncated ? "（文件被截断）" : ""}`,
    );
    if (first && last) {
      lines.push(`时间范围：+0.000s – ${formatDelta(last.ts - first.ts)}（相对首条报文）`);
    }
    lines.push(`设备（${deviceList.length}）：`);
    lines.push(...(deviceList.length > 0 ? deviceList.map(deviceLine) : ["  抓包里没有设备流量"]));
    lines.push(`协议：${options.protocol}`);
    lines.push(
      filtered.length > 0
        ? `报文：${filtered.length} 条（${[...counts.entries()]
            .sort((left, right) => right[1] - left[1])
            .slice(0, 8)
            .map(([kind, count]) => `${kind} ${count}`)
            .join("，")}）`
        : "报文：没有识别到该协议的报文。若设备类别判断有误，用 protocol=raw 看原始报文。",
    );
    const withAnomalies: Array<{ position: number; message: DecodedMessage }> = [];
    filtered.forEach((message, position) => {
      if (message.anomalies.length > 0) withAnomalies.push({ position: position + 1, message });
    });
    lines.push(`异常：${anomalyCount} 处${withAnomalies.length > 0 ? `（${withAnomalies.length} 条报文）` : ""}`);
    for (const entry of withAnomalies.slice(0, MAX_ANOMALIES_LISTED)) {
      lines.push(`  #${entry.position} ${entry.message.anomalies[0]}`);
    }
    if (withAnomalies.length > MAX_ANOMALIES_LISTED) {
      lines.push(`  另有 ${withAnomalies.length - MAX_ANOMALIES_LISTED} 条报文含异常，用 onlyErrors 过滤查看。`);
    }
    const uniqueNotes = [...new Set(notes)];
    if (uniqueNotes.length > 0) {
      lines.push("提示：");
      lines.push(...uniqueNotes.slice(0, MAX_NOTES).map((note) => `  ${note}`));
    }
    return {
      text: lines.join("\n"),
      meta: {
        view: "summary",
        protocol: options.protocol,
        devices: deviceList.length,
        messages: filtered.length,
        anomalies: anomalyCount,
        truncatedFile: prepared.truncated,
        cursor: null,
        truncated: false,
      },
    };
  }

  if (options.view === "timeline") {
    const limit = Math.min(
      Math.max(1, options.limit ?? DEFAULT_TIMELINE_LIMIT),
      MAX_TIMELINE_LIMIT,
    );
    const from = Number.parseInt(options.cursor ?? "1", 10);
    const startPosition = Number.isFinite(from) && from > 1 ? from - 1 : 0;
    const page = filtered.slice(startPosition, startPosition + limit);
    // 相对首条报文的时间偏移：抓包时间戳有的是开机微秒（usbmon）有的是墙钟（USBPcap），
    // 绝对值对协议分析没有意义，而 +0.003s 这种相对时序才是「谁在前谁在后」的依据。
    const origin = filtered[0]?.ts ?? 0;
    const lines = page.map((message, offset) => {
      const index = startPosition + offset + 1;
      const suffix = message.anomalies.length > 0 ? `  ⚠ ${message.anomalies.join("；")}` : "";
      return `#${index} ${formatDelta(message.ts - origin)} ${message.busDev} ${message.kind} — ${message.summary}${suffix}`;
    });
    const nextCursor = startPosition + page.length < filtered.length ? startPosition + page.length + 1 : null;
    const header = `报文 ${startPosition + 1}–${startPosition + page.length} / 共 ${filtered.length} 条`;
    const uniqueNotes = [...new Set(notes)];
    return {
      text: [
        header,
        ...(lines.length > 0 ? lines : ["（没有匹配的报文）"]),
        ...(nextCursor ? [`下一页：cursor=${nextCursor}`] : []),
        ...(uniqueNotes.length > 0 ? ["提示：", ...uniqueNotes.slice(0, MAX_NOTES).map((note) => `  ${note}`)] : []),
      ].join("\n"),
      meta: {
        view: "timeline",
        protocol: options.protocol,
        messages: filtered.length,
        shown: page.length,
        cursor: nextCursor,
        truncated: nextCursor !== null,
      },
    };
  }

  const index = options.index ?? 0;
  const message = index >= 1 ? filtered[index - 1] : undefined;
  if (!message) {
    return {
      text:
        `没有编号为 #${index} 的报文：本次过滤后共 ${filtered.length} 条` +
        `${filtered.length > 0 ? `（可用范围 1–${filtered.length}）` : ""}。先用 view=timeline 取编号。`,
      meta: { view: "message", protocol: options.protocol, messages: filtered.length, found: false },
    };
  }
  const payloadLimit = Math.min(
    Math.max(1, options.payloadBytes ?? DEFAULT_PAYLOAD_BYTES),
    MAX_PAYLOAD_BYTES,
  );
  const payload = message.payload ?? new Uint8Array(0);
  return {
    text: [
      `#${index} ${formatDelta(message.ts - (filtered[0]?.ts ?? 0))} ${message.busDev} ${message.direction.toUpperCase()} ${message.kind}`,
      ...message.detail.map((line) => `  ${line}`),
      ...(payload.length > 0
        ? [`  正文（${payload.length} 字节，截取前 ${Math.min(payloadLimit, payload.length)} 字节）：${toHex(payload, payloadLimit)}`]
        : ["  正文：无"]),
      ...(message.anomalies.length > 0 ? [`  异常：${message.anomalies.join("；")}`] : []),
      `  归属报文：${message.exchanges.map((value) => `#${value}`).join(", ")}`,
    ].join("\n"),
    meta: {
      view: "message",
      protocol: options.protocol,
      messages: filtered.length,
      found: true,
      payloadBytes: Math.min(payloadLimit, payload.length),
    },
  };
}

function collectAddresses(
  exchanges: UsbExchange[],
): Map<string, { busDev: string; bus: number; device: number }> {
  const addresses = new Map<string, { busDev: string; bus: number; device: number }>();
  for (const exchange of exchanges) {
    if (!addresses.has(exchange.busDev)) {
      addresses.set(exchange.busDev, {
        busDev: exchange.busDev,
        bus: exchange.bus,
        device: exchange.device,
      });
    }
  }
  return addresses;
}
