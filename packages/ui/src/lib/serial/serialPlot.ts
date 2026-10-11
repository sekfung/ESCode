import type { SerialChunk } from "@escode/services";
import {
  createSerialStreamDecoder,
  type SerialDisplayEncoding,
  type SerialStreamDecoder,
} from "@/lib/serial/serialFormat.js";

/**
 * 串口波形（docs/specs/serial-port-debugger-phase3.md 第 6 节）：从 RX 文本按行解析数值。
 * 只在渲染层进行，不影响 Host 缓冲与日志导出。
 */
export const SERIAL_PLOT_MAX_SERIES = 8;
export const SERIAL_PLOT_MAX_POINTS = 2000;

export type SerialPlotRow = { t: number } & Record<string, number>;

const NUMBER_PATTERN = /^[-+]?(\d+\.?\d*|\.\d+)([eE][-+]?\d+)?$/;
const KEY_VALUE_PATTERN = /^([^:=]+)[:=](.*)$/;

function parseNumber(token: string): number | null {
  return NUMBER_PATTERN.test(token) ? Number(token) : null;
}

/**
 * 一行包含 `名称:值` / `名称=值` 时按键值对解析，否则按逗号/空白/制表符分隔的数字列（ch1、ch2…）。
 * 无法解析的值跳过；数字列中被跳过的值仍占列号，保证后续列名稳定。
 */
export function parseSerialPlotLine(line: string): Array<[string, number]> {
  const tokens = line.split(/[\s,]+/).filter(Boolean);
  if (tokens.some((token) => KEY_VALUE_PATTERN.test(token))) {
    const points: Array<[string, number]> = [];
    for (const token of tokens) {
      const match = KEY_VALUE_PATTERN.exec(token);
      const value = match ? parseNumber(match[2]!) : null;
      if (match && value !== null) points.push([match[1]!, value]);
    }
    return points;
  }
  const points: Array<[string, number]> = [];
  tokens.forEach((token, index) => {
    const value = parseNumber(token);
    if (value !== null) points.push([`ch${index + 1}`, value]);
  });
  return points;
}

/**
 * 增量解析：每次传入当前完整 chunk 列表，只处理 seq 大于已处理位置的 RX chunk。
 * 列表为空或最大 seq 回退（缓冲被清空）时重置。
 */
export class SerialPlotBuffer {
  series: string[] = [];
  rows: SerialPlotRow[] = [];
  private decoder: SerialStreamDecoder;
  private remainder = "";
  private lastSeq = 0;
  private origin: number | null = null;
  private seriesRows = new Map<string, SerialPlotRow[]>();

  constructor(private readonly encoding: SerialDisplayEncoding) {
    this.decoder = createSerialStreamDecoder(encoding);
  }

  reset(): void {
    this.series = [];
    this.rows = [];
    this.decoder = createSerialStreamDecoder(this.encoding);
    this.remainder = "";
    this.lastSeq = 0;
    this.origin = null;
    this.seriesRows.clear();
  }

  /** 返回是否有新数据点。 */
  push(chunks: readonly SerialChunk[]): boolean {
    const latest = chunks.at(-1)?.seq ?? 0;
    if (latest < this.lastSeq || chunks.length === 0) {
      const hadData = this.rows.length > 0;
      this.reset();
      if (chunks.length === 0) return hadData;
    }
    let changed = false;
    for (const chunk of chunks) {
      if (chunk.seq <= this.lastSeq) continue;
      this.lastSeq = chunk.seq;
      if (chunk.direction !== "rx") continue;
      const lines = (this.remainder + this.decoder.decode(chunk.bytes)).split("\n");
      this.remainder = lines.pop() ?? "";
      for (const line of lines) {
        if (this.appendLine(line.replace(/\r$/, ""), chunk.at)) changed = true;
      }
    }
    return changed;
  }

  /** 行时间取该行结束所在 chunk 的 Host 时间戳。 */
  private appendLine(line: string, at: number): boolean {
    const points = parseSerialPlotLine(line).filter(([name]) => {
      if (this.series.includes(name)) return true;
      if (this.series.length >= SERIAL_PLOT_MAX_SERIES) return false;
      this.series.push(name);
      return true;
    });
    if (points.length === 0) return false;
    this.origin ??= at;
    const row = { t: (at - this.origin) / 1000 } as SerialPlotRow;
    for (const [name, value] of points) {
      row[name] = value;
      const owned = this.seriesRows.get(name) ?? [];
      owned.push(row);
      this.seriesRows.set(name, owned);
      if (owned.length > SERIAL_PLOT_MAX_POINTS) {
        const evicted = owned.shift()!;
        delete evicted[name];
      }
    }
    this.rows.push(row);
    // 只剩 t 的行已无数据点；被淘汰的行都在前部，从头部清理即可。
    while (this.rows.length > 0 && Object.keys(this.rows[0]!).length === 1) this.rows.shift();
    return true;
  }
}

export function buildSerialPlotCsv(
  series: readonly string[],
  rows: readonly SerialPlotRow[],
): string {
  const lines = [["t", ...series].join(",")];
  for (const row of rows) {
    lines.push([row.t, ...series.map((name) => row[name] ?? "")].join(","));
  }
  return `${lines.join("\n")}\n`;
}
