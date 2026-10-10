import type { SerialChunk } from "@zcode/services";
import { verifySerialChecksum, type SerialChecksumConfig } from "@zcode/shared/serial";
import {
  formatSerialHex,
  parseSerialHexInput,
  type SerialDisplayEncoding,
  type SerialDisplayRow,
  type SerialSendMode,
} from "@/lib/serial/serialFormat.js";

/**
 * 接收分帧与帧校验（docs/specs/serial-port-debugger-phase3.md 第 5 节）。只在渲染层对 RX 生效，
 * 不改变 Host 缓冲与导出；TX 每个 chunk 一行保持原样，且不打断正在拼接的 RX 帧。
 */
export type SerialFramingConfig =
  | { mode: "delimiter"; delimiter: Uint8Array }
  | { mode: "length"; length: number }
  | { mode: "gap"; gapMs: number };

export type SerialFramingMode = SerialFramingConfig["mode"];

export interface SerialFramingInputs {
  mode: SerialFramingMode;
  delimiter: string;
  delimiterMode: SerialSendMode;
  length: string;
  gapMs: string;
}

export const DEFAULT_SERIAL_FRAMING_INPUTS: SerialFramingInputs = {
  mode: "delimiter",
  delimiter: "\\r\\n",
  delimiterMode: "text",
  length: "8",
  gapMs: "20",
};

const MAX_FRAME_LENGTH = 4096;
const MAX_GAP_MS = 60_000;

export type SerialFramingParseResult =
  | { ok: true; framing: SerialFramingConfig }
  | { ok: false; error: "delimiter" | "length" | "gap" };

const TEXT_ESCAPES: Record<string, number> = { r: 0x0d, n: 0x0a, t: 0x09, "\\": 0x5c, "0": 0x00 };

/** 文本分隔符：支持 \r \n \t \0 \\ 与 \xNN，其余字符按 UTF-8 编码。 */
function parseEscapedDelimiter(input: string): Uint8Array | null {
  const bytes: number[] = [];
  const encoder = new TextEncoder();
  for (let index = 0; index < input.length; index += 1) {
    const char = input[index]!;
    if (char !== "\\") {
      const codePoint = input.codePointAt(index)!;
      const literal = String.fromCodePoint(codePoint);
      bytes.push(...encoder.encode(literal));
      index += literal.length - 1;
      continue;
    }
    const next = input[index + 1];
    if (next === "x") {
      const hex = input.slice(index + 2, index + 4);
      if (!/^[0-9a-fA-F]{2}$/.test(hex)) return null;
      bytes.push(Number.parseInt(hex, 16));
      index += 3;
    } else if (next !== undefined && next in TEXT_ESCAPES) {
      bytes.push(TEXT_ESCAPES[next]!);
      index += 1;
    } else {
      return null;
    }
  }
  return Uint8Array.from(bytes);
}

function parseBoundedInt(value: string, max: number): number | null {
  const trimmed = value.trim();
  if (!/^\d+$/.test(trimmed)) return null;
  const parsed = Number(trimmed);
  return parsed >= 1 && parsed <= max ? parsed : null;
}

export function parseSerialFramingInputs(inputs: SerialFramingInputs): SerialFramingParseResult {
  switch (inputs.mode) {
    case "delimiter": {
      const parsed =
        inputs.delimiterMode === "hex"
          ? parseSerialHexInput(inputs.delimiter)
          : { ok: true as const, bytes: parseEscapedDelimiter(inputs.delimiter) };
      const delimiter = parsed.ok ? parsed.bytes : null;
      if (!delimiter || delimiter.byteLength === 0) return { ok: false, error: "delimiter" };
      return { ok: true, framing: { mode: "delimiter", delimiter } };
    }
    case "length": {
      const length = parseBoundedInt(inputs.length, MAX_FRAME_LENGTH);
      return length === null
        ? { ok: false, error: "length" }
        : { ok: true, framing: { mode: "length", length } };
    }
    case "gap": {
      const gapMs = parseBoundedInt(inputs.gapMs, MAX_GAP_MS);
      return gapMs === null
        ? { ok: false, error: "gap" }
        : { ok: true, framing: { mode: "gap", gapMs } };
    }
  }
}

export interface SerialFramedRow extends SerialDisplayRow {
  /** 尾部尚未遇到帧结束的 RX 数据；不参与校验。 */
  partial?: boolean;
  checksum?: { ok: boolean; expected: string; actual: string };
}

interface PendingFrame {
  key: string;
  at: number;
  source: SerialChunk["source"];
  bytes: number[];
}

export function buildSerialFramedRows(
  chunks: readonly SerialChunk[],
  options: {
    mode: SerialSendMode;
    encoding: SerialDisplayEncoding;
    framing: SerialFramingConfig;
    verify?: SerialChecksumConfig;
  },
): SerialFramedRow[] {
  const { framing } = options;
  const render = (bytes: Uint8Array) =>
    options.mode === "hex"
      ? formatSerialHex(bytes)
      : new TextDecoder(options.encoding, { fatal: false }).decode(bytes);
  const rows: SerialFramedRow[] = [];
  let pending: PendingFrame | null = null;
  let lastRxAt: number | null = null;

  const emit = (frame: PendingFrame, partial: boolean) => {
    const bytes = Uint8Array.from(frame.bytes);
    rows.push({
      key: frame.key,
      direction: "rx",
      source: frame.source,
      at: frame.at,
      text: render(bytes),
      ...(partial ? { partial: true } : {}),
      ...(!partial && options.verify
        ? { checksum: verifySerialChecksum(bytes, options.verify) }
        : {}),
    });
  };

  for (const chunk of chunks) {
    if (chunk.direction === "tx") {
      rows.push({
        key: String(chunk.seq),
        direction: chunk.direction,
        source: chunk.source,
        ...(chunk.sessionId ? { sessionId: chunk.sessionId } : {}),
        at: chunk.at,
        text: render(chunk.bytes),
      });
      continue;
    }
    if (framing.mode === "gap" && pending && lastRxAt !== null) {
      if (chunk.at - lastRxAt > framing.gapMs) {
        emit(pending, false);
        pending = null;
      }
    }
    lastRxAt = chunk.at;
    for (let index = 0; index < chunk.bytes.byteLength; index += 1) {
      // 帧 key 取起始 chunk 的 seq 与帧内序号，同一 chunk 可能开启多帧。
      pending ??= { key: `${chunk.seq}:${index}`, at: chunk.at, source: chunk.source, bytes: [] };
      pending.bytes.push(chunk.bytes[index]!);
      if (framing.mode === "length" && pending.bytes.length >= framing.length) {
        emit(pending, false);
        pending = null;
      } else if (framing.mode === "delimiter" && endsWith(pending.bytes, framing.delimiter)) {
        pending.bytes.length -= framing.delimiter.byteLength;
        emit(pending, false);
        pending = null;
      }
    }
  }
  // 间隔模式没有结束标记：尾帧视为完整，数据到达即可校验，无需等待下一帧。
  if (pending) emit(pending, framing.mode !== "gap");
  return rows;
}

function endsWith(bytes: readonly number[], suffix: Uint8Array): boolean {
  if (bytes.length < suffix.byteLength) return false;
  const offset = bytes.length - suffix.byteLength;
  return suffix.every((byte, index) => bytes[offset + index] === byte);
}
