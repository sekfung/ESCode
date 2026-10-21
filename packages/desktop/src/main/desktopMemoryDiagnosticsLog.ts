import {
  createMemorySampleWriteGate,
  formatMemorySampleLine,
  MEMORY_SAMPLE_INTERVAL_MS,
  memoryUsageToSampleFields,
  type MemorySample,
} from "@escode/shared";
import { mainMemoryDiagnosticsRegistry } from "./mainMemoryDiagnostics.js";

interface MemoryDiagnosticsLogger {
  info(...args: unknown[]): void;
}

let handle: ReturnType<typeof setInterval> | undefined;
let gate = createMemorySampleWriteGate();

/** main 进程每 60 秒写一行本地内存诊断；写盘门控后调用。 */
export function startDesktopMemoryDiagnosticsLog(logger: MemoryDiagnosticsLogger): void {
  if (handle !== undefined) return;
  const sampleNow = () => {
    try {
      const memoryUsage = process.memoryUsage();
      const sample: MemorySample = {
        role: "main",
        ...memoryUsageToSampleFields(memoryUsage),
        counters: mainMemoryDiagnosticsRegistry.collect(),
      };
      const reason = gate.evaluate(sample, Date.now());
      if (!reason) return;
      logger.info(formatMemorySampleLine(sample, reason));
    } catch {
      // 本地诊断采样失败只丢当前样本，不影响主进程。
    }
  };
  handle = setInterval(sampleNow, MEMORY_SAMPLE_INTERVAL_MS);
  handle.unref?.();
}

export function stopDesktopMemoryDiagnosticsLog(): void {
  if (handle === undefined) return;
  clearInterval(handle);
  handle = undefined;
  gate = createMemorySampleWriteGate();
}
