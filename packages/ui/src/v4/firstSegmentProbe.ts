/**
 * 首段分阶段计时探针（仅 E2E 构建启用）。
 *
 * 目标：把「submit → 首个会话帧 → 首个投影行 → 首个 assistant 文本」拆开，定位冷态首条
 * 150–230 ms 的固定开销。生产构建不注册任何全局对象；只有
 * `VITE_ESCODE_E2E_STORE_BRIDGE=1` 的 E2E 构建才暴露 `window.__ESCODE_FIRST_SEGMENT_PROBE__`。
 */

export interface FirstSegmentProbeSnapshot {
  armedAt: number;
  firstFrameAt: number;
  firstRowAt: number;
}

type ProbeWindow = Window & {
  __ESCODE_FIRST_SEGMENT_PROBE__?: {
    arm: () => number;
    read: () => FirstSegmentProbeSnapshot | null;
  };
};

let snapshot: FirstSegmentProbeSnapshot | null = null;

function now(): number {
  return typeof performance === "undefined" ? 0 : performance.now();
}

export function armFirstSegmentProbe(): number {
  snapshot = { armedAt: now(), firstFrameAt: 0, firstRowAt: 0 };
  return snapshot.armedAt;
}

export function markFirstSegmentFrame(): void {
  if (snapshot && snapshot.firstFrameAt === 0) {
    snapshot.firstFrameAt = now();
  }
}

export function markFirstSegmentRowApplied(): void {
  if (snapshot && snapshot.firstRowAt === 0) {
    snapshot.firstRowAt = now();
  }
}

export function readFirstSegmentProbe(): FirstSegmentProbeSnapshot | null {
  return snapshot ? { ...snapshot } : null;
}

export function installFirstSegmentProbeBridge(): void {
  if (typeof window === "undefined") return;
  const env = (import.meta as ImportMeta & { env?: Record<string, string | boolean> }).env;
  if (env?.VITE_ESCODE_E2E_STORE_BRIDGE !== "1") return;
  (window as ProbeWindow).__ESCODE_FIRST_SEGMENT_PROBE__ = {
    arm: armFirstSegmentProbe,
    read: readFirstSegmentProbe,
  };
}
