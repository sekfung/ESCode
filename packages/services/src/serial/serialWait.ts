import type { Event } from "@escode/rpc";
import type { SerialChunk, SerialStatus } from "./serial.js";

export type SerialWaitResult<T> =
  | { kind: "matched"; value: T; lastSeq: number }
  | { kind: "timeout" | "disconnected" | "aborted"; lastSeq: number };

type WaitOutcome<T> =
  | { kind: "matched"; value: T }
  | { kind: "timeout" | "disconnected" | "aborted" };

/**
 * Agent serial_wait_for 的等待实现：在 sinceSeq（缺省为调用时刻）之后的 RX 上反复执行 test。
 * 先检查已有数据，再订阅新数据与状态；串口不处于 open、超时或取消时结束，并且一定释放订阅。
 */
export function waitForSerialRx<T>(
  deps: {
    lastSeq: () => number;
    rxSince: (seq: number) => SerialChunk[];
    isOpen: () => boolean;
    onData: Event<SerialChunk>;
    onStatus: Event<SerialStatus>;
  },
  params: {
    sinceSeq?: number;
    timeoutMs: number;
    signal?: AbortSignal;
    test: (rxChunks: readonly SerialChunk[]) => T | null;
  },
): Promise<SerialWaitResult<T>> {
  const sinceSeq = params.sinceSeq ?? deps.lastSeq();
  return new Promise((resolve) => {
    let settled = false;
    const subscriptions: Array<{ dispose(): void }> = [];
    let timer: ReturnType<typeof setTimeout> | null = null;
    const onAbort = () => finish({ kind: "aborted" });
    const finish = (result: WaitOutcome<T>) => {
      if (settled) return;
      settled = true;
      if (timer) clearTimeout(timer);
      for (const subscription of subscriptions) subscription.dispose();
      params.signal?.removeEventListener("abort", onAbort);
      resolve({ ...result, lastSeq: deps.lastSeq() });
    };
    const check = () => {
      const value = params.test(deps.rxSince(sinceSeq));
      if (value !== null && value !== undefined) finish({ kind: "matched", value });
    };
    if (params.signal?.aborted) return finish({ kind: "aborted" });
    check();
    if (settled) return;
    if (!deps.isOpen()) return finish({ kind: "disconnected" });
    subscriptions.push(
      deps.onData((chunk) => {
        if (chunk.direction === "rx") check();
      }),
      deps.onStatus((status) => {
        if (status.state !== "open") finish({ kind: "disconnected" });
      }),
    );
    params.signal?.addEventListener("abort", onAbort);
    timer = setTimeout(() => finish({ kind: "timeout" }), params.timeoutMs);
  });
}
