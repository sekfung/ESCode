import type { SerialLoopState } from "./serial.js";

/** 进度事件最短间隔：高频循环不能把状态事件刷屏（启动、停止立即推送）。 */
const PROGRESS_THROTTLE_MS = 250;

/**
 * 串口定时循环发送（docs/specs/serial-port-debugger-phase3.md 第 4 节），由 Host 会话持有：
 * 立即发送第一次，之后按间隔发送；上一笔写入未完成时跳过本次 tick，不排队积压；
 * 达到次数、写入失败或被 stop 时结束。
 */
export class SerialLoopRunner {
  private sent = 0;
  private busy = false;
  private stopped = false;
  private timer: ReturnType<typeof setInterval> | null = null;
  private lastProgressAt = 0;

  constructor(
    private readonly options: {
      bytes: Uint8Array;
      intervalMs: number;
      count?: number;
      write: (bytes: Uint8Array) => Promise<unknown>;
      onProgress: (state: SerialLoopState) => void;
      /** 自然结束（达到次数或写入失败）；stop() 主动停止不回调。 */
      onFinished: () => void;
    },
  ) {}

  get state(): SerialLoopState {
    return {
      intervalMs: this.options.intervalMs,
      ...(this.options.count !== undefined ? { count: this.options.count } : {}),
      sent: this.sent,
    };
  }

  start(): void {
    this.lastProgressAt = Date.now();
    this.options.onProgress(this.state);
    this.timer = setInterval(() => void this.tick(), this.options.intervalMs);
    void this.tick();
  }

  stop(): void {
    this.stopped = true;
    if (this.timer) clearInterval(this.timer);
    this.timer = null;
  }

  private async tick(): Promise<void> {
    if (this.busy || this.stopped) return;
    this.busy = true;
    try {
      await this.options.write(this.options.bytes);
      this.sent += 1;
    } catch {
      this.finish();
      return;
    } finally {
      this.busy = false;
    }
    if (this.stopped) return;
    if (this.options.count !== undefined && this.sent >= this.options.count) {
      this.finish();
      return;
    }
    const now = Date.now();
    if (now - this.lastProgressAt >= PROGRESS_THROTTLE_MS) {
      this.lastProgressAt = now;
      this.options.onProgress(this.state);
    }
  }

  private finish(): void {
    if (this.stopped) return;
    this.stop();
    this.options.onFinished();
  }
}
