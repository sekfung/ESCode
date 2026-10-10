import type { SerialChunk, SerialSnapshot, SerialStats } from "@escode/services";

/** 与 Host 环形缓冲上限一致；渲染侧只是镜像，不是串口数据的事实来源。 */
const DEFAULT_LIMIT_BYTES = 1024 * 1024;

export interface SerialChunkLog {
  readonly limitBytes: number;
  readonly snapshotLoaded: boolean;
  /** 快照到达前收到的 onData，等快照后按 seq 去重合并。 */
  readonly pending: readonly SerialChunk[];
  readonly chunks: readonly SerialChunk[];
  readonly bytes: number;
  readonly lastSeq: number;
  /** 以快照计数为基准，只累加快照之后追加的 chunk。 */
  readonly stats: SerialStats;
}

export function createSerialChunkLog(options: { limitBytes?: number } = {}): SerialChunkLog {
  return {
    limitBytes: options.limitBytes ?? DEFAULT_LIMIT_BYTES,
    snapshotLoaded: false,
    pending: [],
    chunks: [],
    bytes: 0,
    lastSeq: 0,
    stats: { rxBytes: 0, txBytes: 0 },
  };
}

function append(
  log: SerialChunkLog,
  incoming: readonly SerialChunk[],
  options: { countStats: boolean },
): SerialChunkLog {
  const chunks = [...log.chunks];
  let bytes = log.bytes;
  let lastSeq = log.lastSeq;
  const stats = { ...log.stats };
  for (const chunk of incoming) {
    if (chunk.seq <= lastSeq) continue;
    chunks.push(chunk);
    bytes += chunk.bytes.byteLength;
    lastSeq = chunk.seq;
    if (!options.countStats) continue;
    if (chunk.direction === "rx") stats.rxBytes += chunk.bytes.byteLength;
    else stats.txBytes += chunk.bytes.byteLength;
  }
  while (bytes > log.limitBytes && chunks.length > 1) {
    const evicted = chunks.shift();
    if (evicted) bytes -= evicted.bytes.byteLength;
  }
  return { ...log, chunks, bytes, lastSeq, stats };
}

/**
 * 先订阅再取快照：快照前的事件暂存在 pending，快照到达后丢弃 seq ≤ 快照 seq 的部分，
 * 不依赖延时即可保证不重复、不遗漏。
 */
export function applySerialSnapshot(log: SerialChunkLog, snapshot: SerialSnapshot): SerialChunkLog {
  const base: SerialChunkLog = {
    ...log,
    snapshotLoaded: true,
    pending: [],
    chunks: [],
    bytes: 0,
    lastSeq: 0,
    stats: { ...snapshot.stats },
  };
  // 快照内 chunk 已计入 snapshot.stats，不能重复累加。
  const withSnapshot = append(base, snapshot.chunks, { countStats: false });
  return append(
    { ...withSnapshot, lastSeq: Math.max(withSnapshot.lastSeq, snapshot.seq) },
    log.pending,
    { countStats: true },
  );
}

export function applySerialChunk(log: SerialChunkLog, chunk: SerialChunk): SerialChunkLog {
  if (!log.snapshotLoaded) return { ...log, pending: [...log.pending, chunk] };
  return append(log, [chunk], { countStats: true });
}

/** 本地清屏：保留 lastSeq，避免之后的旧事件被重新追加。 */
export function clearSerialChunkLog(log: SerialChunkLog): SerialChunkLog {
  return { ...log, chunks: [], bytes: 0, stats: { rxBytes: 0, txBytes: 0 } };
}
