import type { SerialChunk, SerialDirection, SerialSource, SerialStats } from "./serial.js";

export interface SerialReadResult {
  chunks: SerialChunk[];
  /** 下次读取应传入的游标；越过被方向过滤掉的 chunk。 */
  lastSeq: number;
  truncated: boolean;
  /** 游标之后有数据已被淘汰或清空，读到的内容不连续。 */
  evicted: boolean;
}

interface PendingRx {
  at: number;
  parts: Buffer[];
  length: number;
  timer: ReturnType<typeof setTimeout> | null;
}

/**
 * 串口收发记录：RX 合并、seq 分配与按字节上限的环形淘汰。
 * 由所属串口会话独占持有。seq 由 SerialService 统一分配（窗口内跨会话单调递增、clear 与重开都不回退），
 * 因此同一串口关闭后重开也不会让 renderer 把新数据误判为快照内旧数据；同一缓冲内的 seq 可以不连续。
 */
export class SerialChunkBuffer {
  private chunks: SerialChunk[] = [];
  private bufferedBytes = 0;
  /** 本缓冲最后分配到的 seq。 */
  private seq = 0;
  /** 已被淘汰或清空的最大 seq：游标早于它说明中间有数据缺失。 */
  private evictedThrough = 0;
  private stats: SerialStats = { rxBytes: 0, txBytes: 0 };
  private pendingRx: PendingRx | null = null;

  constructor(
    private readonly options: {
      limitBytes: number;
      /** 相邻 RX 合并窗口；≤0 时每次收到数据立即成段。 */
      coalesceWindowMs: number;
      coalesceMaxBytes: number;
      onChunk: (chunk: SerialChunk) => void;
      /** 共享的 seq 分配器；缺省为缓冲内自增（单会话测试使用）。 */
      nextSeq?: () => number;
    },
  ) {}

  private localSeq = 0;
  private allocateSeq(): number {
    return this.options.nextSeq ? this.options.nextSeq() : ++this.localSeq;
  }

  receive(data: Buffer): void {
    if (data.length === 0) return;
    if (this.options.coalesceWindowMs <= 0) {
      this.push("rx", "user", data);
      return;
    }
    if (this.pendingRx && this.pendingRx.length + data.length > this.options.coalesceMaxBytes) {
      this.flushRx();
    }
    this.pendingRx ??= { at: Date.now(), parts: [], length: 0, timer: null };
    const pending = this.pendingRx;
    pending.parts.push(data);
    pending.length += data.length;
    if (pending.timer) clearTimeout(pending.timer);
    pending.timer = setTimeout(() => this.flushRx(), this.options.coalesceWindowMs);
  }

  /** TX 在提交写入时入缓冲：先落下尚未合并完的 RX，保证回显出现在设备对它的响应之前。 */
  pushTx(source: SerialSource, bytes: Buffer, sessionId?: string): void {
    this.flushRx();
    this.push("tx", source, bytes, Date.now(), sessionId);
  }

  get lastSeq(): number {
    return this.seq;
  }

  /**
   * 游标增量读取（Agent serial_read）。未给游标时只返回当前 lastSeq，让调用方从“此刻”开始读。
   * 单个 chunk 超过 maxBytes 时返回前缀并越过该 chunk，避免游标原地踏步。
   */
  readSince(params: {
    sinceSeq?: number;
    direction: SerialDirection | "both";
    maxBytes: number;
  }): SerialReadResult {
    const { sinceSeq, direction, maxBytes } = params;
    if (sinceSeq === undefined) {
      return { chunks: [], lastSeq: this.seq, truncated: false, evicted: false };
    }
    const evicted = sinceSeq < this.evictedThrough;
    const chunks: SerialChunk[] = [];
    let lastSeq = Math.max(sinceSeq, Math.min(this.seq, this.evictedThrough));
    let bytes = 0;
    for (const chunk of this.chunks) {
      if (chunk.seq <= sinceSeq) continue;
      if (direction !== "both" && chunk.direction !== direction) {
        lastSeq = chunk.seq;
        continue;
      }
      if (bytes + chunk.bytes.byteLength > maxBytes) {
        if (chunks.length === 0) {
          chunks.push({ ...chunk, bytes: chunk.bytes.subarray(0, maxBytes) });
          lastSeq = chunk.seq;
        }
        return { chunks, lastSeq, truncated: true, evicted };
      }
      chunks.push(chunk);
      bytes += chunk.bytes.byteLength;
      lastSeq = chunk.seq;
    }
    return { chunks, lastSeq: Math.max(lastSeq, this.seq), truncated: false, evicted };
  }

  /** 游标之后的 RX chunk（Agent serial_wait_for 的匹配输入）。 */
  rxSince(sinceSeq: number): SerialChunk[] {
    return this.chunks.filter((chunk) => chunk.seq > sinceSeq && chunk.direction === "rx");
  }

  flushRx(): void {
    const pending = this.pendingRx;
    if (!pending) return;
    this.pendingRx = null;
    if (pending.timer) clearTimeout(pending.timer);
    this.push("rx", "user", Buffer.concat(pending.parts, pending.length), pending.at);
  }

  clear(): void {
    this.evictedThrough = this.seq;
    this.chunks = [];
    this.bufferedBytes = 0;
    this.stats = { rxBytes: 0, txBytes: 0 };
  }

  snapshot(): { chunks: SerialChunk[]; seq: number; stats: SerialStats } {
    return {
      chunks: [...this.chunks],
      seq: this.seq,
      stats: { ...this.stats },
    };
  }

  private push(
    direction: SerialChunk["direction"],
    source: SerialSource,
    bytes: Buffer,
    at = Date.now(),
    sessionId?: string,
  ): void {
    this.seq = this.allocateSeq();
    const chunk: SerialChunk = {
      seq: this.seq,
      at,
      direction,
      source,
      ...(sessionId ? { sessionId } : {}),
      bytes: new Uint8Array(bytes.buffer, bytes.byteOffset, bytes.byteLength),
    };
    this.chunks.push(chunk);
    this.bufferedBytes += bytes.byteLength;
    if (direction === "rx") this.stats.rxBytes += bytes.byteLength;
    else this.stats.txBytes += bytes.byteLength;
    while (this.bufferedBytes > this.options.limitBytes && this.chunks.length > 1) {
      const evicted = this.chunks.shift();
      if (evicted) {
        this.bufferedBytes -= evicted.bytes.byteLength;
        this.evictedThrough = evicted.seq;
      }
    }
    this.options.onChunk(chunk);
  }
}
