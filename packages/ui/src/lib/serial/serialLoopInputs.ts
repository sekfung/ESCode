/** 与 Host 一致的最小间隔（services SERIAL_LOOP_MIN_INTERVAL_MS）。 */
export const SERIAL_LOOP_MIN_INTERVAL_MS = 10;

export type SerialLoopInputs =
  | { ok: true; intervalMs: number; count?: number }
  | { ok: false; error: "interval" | "count" };

const INTEGER = /^\d+$/;

/** 解析发送栏的循环参数：次数留空表示无限循环。 */
export function parseSerialLoopInputs(intervalText: string, countText: string): SerialLoopInputs {
  const interval = intervalText.trim();
  if (!INTEGER.test(interval) || Number(interval) < SERIAL_LOOP_MIN_INTERVAL_MS) {
    return { ok: false, error: "interval" };
  }
  const count = countText.trim();
  if (count === "") return { ok: true, intervalMs: Number(interval) };
  if (!INTEGER.test(count) || Number(count) < 1) return { ok: false, error: "count" };
  return { ok: true, intervalMs: Number(interval), count: Number(count) };
}
