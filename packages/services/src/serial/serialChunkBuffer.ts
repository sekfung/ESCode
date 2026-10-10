import type { SerialChunk, SerialSource, SerialStats } from "./serial.js";

interface PendingRx {
  at: number;
  parts: Buffer[];
  length: number;
  timer: ReturnType<typeof setTimeout> | null;
}

/**
 * 串口收发记录：RX 合并、seq 分配与按字节上限的环形淘汰。
 * 由 SerialService 独占持有；seq 单调递增且 clear 后不回退，供 renderer 做快照与事件去重。
 */
export class SerialChunkBuffer {
  private chunks: SerialChunk[] = [];
  private bufferedBytes = 0;
  private seq = 0;
  private stats: SerialStats = { rxBytes: 0, txBytes: 0 };
  private pendingRx: PendingRx | null = null;

  constructor(
    private readonly options: {
      limitBytes: number;
      /** 相邻 RX 合并窗口；≤0 时每次收到数据立即成段。 */
      coalesceWindowMs: number;
      coalesceMaxBytes: number;
      onChunk: (chunk: SerialChunk) => void;
    },
  ) {}

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
  pushTx(source: SerialSource, bytes: Buffer): void {
    this.flushRx();
    this.push("tx", source, bytes);
  }

  flushRx(): void {
    const pending = this.pendingRx;
    if (!pending) return;
    this.pendingRx = null;
    if (pending.timer) clearTimeout(pending.timer);
    this.push("rx", "user", Buffer.concat(pending.parts, pending.length), pending.at);
  }

  clear(): void {
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
  ): void {
    this.seq += 1;
    const chunk: SerialChunk = {
      seq: this.seq,
      at,
      direction,
      source,
      bytes: new Uint8Array(bytes.buffer, bytes.byteOffset, bytes.byteLength),
    };
    this.chunks.push(chunk);
    this.bufferedBytes += bytes.byteLength;
    if (direction === "rx") this.stats.rxBytes += bytes.byteLength;
    else this.stats.txBytes += bytes.byteLength;
    while (this.bufferedBytes > this.options.limitBytes && this.chunks.length > 1) {
      const evicted = this.chunks.shift();
      if (evicted) this.bufferedBytes -= evicted.bytes.byteLength;
    }
    this.options.onChunk(chunk);
  }
}
