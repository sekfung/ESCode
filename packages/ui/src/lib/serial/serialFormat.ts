import type { SerialChunk } from "@zcode/services";

export type SerialSendMode = "text" | "hex";
export type SerialLineEnding = "none" | "cr" | "lf" | "crlf";
export type SerialDisplayEncoding = "utf-8" | "gbk";

export type SerialBytesResult =
  | { ok: true; bytes: Uint8Array }
  | { ok: false; error: "oddLength" | "invalidChar" };

const LINE_ENDINGS: Record<SerialLineEnding, string> = {
  none: "",
  cr: "\r",
  lf: "\n",
  crlf: "\r\n",
};

/** 解析 HEX 输入：允许空白/逗号分隔、大小写混用、每字节可带 0x 前缀；连续书写按两位一组。 */
export function parseSerialHexInput(input: string): SerialBytesResult {
  const tokens = input
    .split(/[\s,]+/)
    .filter(Boolean)
    .map((token) => token.replace(/^0x/i, ""));
  const digits = tokens.join("");
  if (/[^0-9a-f]/i.test(digits)) return { ok: false, error: "invalidChar" };
  if (tokens.some((token) => token.length % 2 !== 0)) return { ok: false, error: "oddLength" };
  const bytes = new Uint8Array(digits.length / 2);
  for (let index = 0; index < bytes.length; index += 1) {
    bytes[index] = Number.parseInt(digits.slice(index * 2, index * 2 + 2), 16);
  }
  return { ok: true, bytes };
}

export function formatSerialHex(bytes: Uint8Array): string {
  return Array.from(bytes, (byte) => byte.toString(16).toUpperCase().padStart(2, "0")).join(" ");
}

/** 文本模式按 UTF-8 编码并追加行尾；HEX 模式按字节原样发送，不追加行尾。 */
export function buildSerialSendPayload(options: {
  input: string;
  mode: SerialSendMode;
  lineEnding: SerialLineEnding;
}): SerialBytesResult {
  if (options.mode === "hex") return parseSerialHexInput(options.input);
  return {
    ok: true,
    bytes: new TextEncoder().encode(options.input + LINE_ENDINGS[options.lineEnding]),
  };
}

export interface SerialStreamDecoder {
  decode(bytes: Uint8Array): string;
}

/** 流式解码：多字节字符跨 chunk 时保留尾部残字节，等下一段到达后再输出。 */
export function createSerialStreamDecoder(encoding: SerialDisplayEncoding): SerialStreamDecoder {
  const decoder = new TextDecoder(encoding, { fatal: false });
  return {
    decode: (bytes) => decoder.decode(bytes, { stream: true }),
  };
}

export interface SerialDisplayRow {
  key: string;
  direction: SerialChunk["direction"];
  source: SerialChunk["source"];
  /** 行内首个 chunk 的时间。 */
  at: number;
  text: string;
}

/**
 * 把 chunk 序列转换为显示行：开启时间戳时每个 chunk 一行；否则同方向相邻 chunk 合并。
 * RX 与 TX 各自使用独立的流式解码器，跨 chunk 的多字节字符不会被拆坏。
 */
export function buildSerialDisplayRows(
  chunks: readonly SerialChunk[],
  options: { mode: SerialSendMode; encoding: SerialDisplayEncoding; showTimestamp: boolean },
): SerialDisplayRow[] {
  const decoders = {
    rx: createSerialStreamDecoder(options.encoding),
    tx: createSerialStreamDecoder(options.encoding),
  };
  const rows: SerialDisplayRow[] = [];
  for (const chunk of chunks) {
    const piece =
      options.mode === "hex"
        ? formatSerialHex(chunk.bytes)
        : decoders[chunk.direction].decode(chunk.bytes);
    const previous = rows.at(-1);
    if (
      !options.showTimestamp &&
      previous &&
      previous.direction === chunk.direction &&
      previous.source === chunk.source
    ) {
      previous.text = options.mode === "hex" ? `${previous.text} ${piece}` : previous.text + piece;
      continue;
    }
    rows.push({
      key: String(chunk.seq),
      direction: chunk.direction,
      source: chunk.source,
      at: chunk.at,
      text: piece,
    });
  }
  return rows;
}

/** 导出为 .log：每个 chunk 一行，带 ISO 时间与方向，便于离线比对。 */
export function buildSerialExportText(
  chunks: readonly SerialChunk[],
  options: { mode: SerialSendMode; encoding: SerialDisplayEncoding },
): string {
  return buildSerialDisplayRows(chunks, { ...options, showTimestamp: true })
    .map((row) => `${new Date(row.at).toISOString()} ${row.direction.toUpperCase()} ${row.text}`)
    .map((line) => (line.endsWith("\n") ? line : `${line}\n`))
    .join("");
}
