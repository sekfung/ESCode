// Session residency 收尾 lease 的 v4 原生实现。
//
// 该 lease 只覆盖 Bootstrap detached runner 的收尾窗口，避免 resident pool 回收仍在使用的
// record。它不持有 prompt 的 busy/idle authority（已归 Core），因此可以在 v4 层直接复用，
// 不再依赖旧协议目录。
export interface SessionResidencyFinalizationOwner {
  residencyFinalizationCount?: number;
}

export function acquireSessionResidencyFinalization(
  record: SessionResidencyFinalizationOwner,
): () => void {
  record.residencyFinalizationCount = (record.residencyFinalizationCount ?? 0) + 1;
  let released = false;
  return () => {
    if (released) return;
    released = true;
    record.residencyFinalizationCount = Math.max(0, (record.residencyFinalizationCount ?? 1) - 1);
  };
}

/** 同步登记 lease，再启动 detached runner；同步抛错和异步终态都保证释放。 */
export function runWithSessionResidencyFinalization<T>(
  record: SessionResidencyFinalizationOwner,
  run: () => Promise<T>,
): Promise<T> {
  const release = acquireSessionResidencyFinalization(record);
  try {
    return run().finally(release);
  } catch (error) {
    release();
    return Promise.reject(error);
  }
}
