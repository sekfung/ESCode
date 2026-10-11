import type { SerialChunk } from "@escode/services";

export {
  buildSerialSendPayload,
  createSerialStreamDecoder,
  formatSerialHex,
  parseSerialHexInput,
  type SerialBytesResult,
  type SerialDisplayEncoding,
  type SerialLineEnding,
  type SerialSendMode,
  type SerialStreamDecoder,
} from "@escode/shared/serial";
import {
  createSerialStreamDecoder,
  formatSerialHex,
  type SerialDisplayEncoding,
  type SerialSendMode,
} from "@escode/shared/serial";

export interface SerialDisplayRow {
  key: string;
  direction: SerialChunk["direction"];
  source: SerialChunk["source"];
  /** source="agent" 时的发起会话；不同会话的写入不合并。 */
  sessionId?: string;
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
      previous.source === chunk.source &&
      previous.sessionId === chunk.sessionId
    ) {
      previous.text = options.mode === "hex" ? `${previous.text} ${piece}` : previous.text + piece;
      continue;
    }
    rows.push({
      key: String(chunk.seq),
      direction: chunk.direction,
      source: chunk.source,
      ...(chunk.sessionId ? { sessionId: chunk.sessionId } : {}),
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
