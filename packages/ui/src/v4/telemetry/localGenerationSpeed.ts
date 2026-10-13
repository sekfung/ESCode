import { estimateTokens, localTtftNow } from "@escode/shared";
import {
  parseConversationTopic,
  type ConversationRow,
  type ConversationSnapshot,
  type ConversationTopicFrame,
  type StreamablePath,
  type TopicFrameDeliveryKind,
  type TurnHeaderRow,
} from "@escode/shared/escode-protocol-v4";

/**
 * 会话**实时接收速率**的本地观测器。
 *
 * 本模块只负责显示矩阵里的"实时值"：3 秒滑窗内估算 token ÷ 窗口时长。它是本机体感指标，
 * 不追求跨设备一致——流式阶段协议不携带逐 chunk token 数，provider 只在每次模型请求 finish 时给
 * usage（见 `core/src/runtime/methods/model.ts`），任何一层的实时值都只能是按流出文本的估算。
 *
 * **轮末解码速度不在这里**：那是会话事实，由 CLI 投影按次调用的"首个生成 delta → 末个生成 delta"
 * 累计到 `turnHeader.decodeMs / decodeTokens`，本观测器只读，不做任何本地结算。
 * 口径见 `docs/specs/conversation-generation-speed.md` 与 `docs/adr/0007-generation-speed-split-ownership.md`。
 *
 * 与 `LocalTtftObserver` 并列：同样只采信 `online` 投递、使用本地单调时钟、恢复/resync 一律排除。
 */

/** 实时窗口长度：窗口内估算 token ÷ 窗口时长（规格 4.1）。 */
export const GENERATION_SPEED_WINDOW_MS = 3_000;
/** 视图刷新节流：读数不得带动 Composer 子树高频重渲染（规格 2.1）。 */
export const GENERATION_SPEED_VIEW_TICK_MS = 500;
/** 会话状态上界；只淘汰既无订阅也不在生成中的会话。 */
const GENERATION_SPEED_MAX_SESSIONS = 32;
/** 行类型登记表上界；`row.delta` 只带 rowId + path，必须靠它判断该 append 是否计入。 */
const GENERATION_SPEED_MAX_TRACKED_ROWS = 512;

export type GenerationSpeedMode = "none" | "live" | "average";

export interface GenerationSpeedView {
  mode: GenerationSpeedMode;
  /** tok/s；`mode === "none"` 时为 null。 */
  value: number | null;
}

/** 未知会话（草稿态、未打开、已淘汰）统一返回同一个对象，保证 useSyncExternalStore 的快照引用稳定。 */
export const NO_GENERATION_SPEED: GenerationSpeedView = Object.freeze({
  mode: "none",
  value: null,
});

/** 只有模型自己吐出的 token 计入速度：正文、思考与工具调用参数。 */
type CountedRowKind = "assistantText" | "reasoning" | "toolCall";

interface SpeedSample {
  at: number;
  tokens: number;
}

interface SpeedRecord {
  sessionId: string;
  /** 窗口样本，按到达时间升序；超窗在刷新时淘汰。 */
  samples: SpeedSample[];
  /** rowId → 流式行类型。 */
  rowKindByRowId: Map<number, CountedRowKind>;
  /** 当前 product turn 的 turnHeader rowId；进度 upsert 与新一轮靠它区分。 */
  turnRowId: number | null;
  turnRunning: boolean;
  /** 本轮首个 delta 的本地时间；用于起步阶段归一，缺口后重置为 null。 */
  segmentStartedAt: number | null;
  settled: boolean;
  /** 轮末解码速度（tok/s），读自 turnHeader 的权威字段；未知时为 null。 */
  average: number | null;
  /** 已发布视图；对象身份只在语义变化时替换。 */
  view: GenerationSpeedView;
  listeners: Set<() => void>;
  timer: ReturnType<typeof setTimeout> | null;
  updatedAt: number;
}

const COUNTED_ROW_KINDS: Record<string, CountedRowKind | undefined> = {
  assistantText: "assistantText",
  reasoning: "reasoning",
  toolCall: "toolCall",
};

/**
 * 轮末解码速度：`decodeTokens ÷ (decodeMs / 1000)`。
 *
 * 两个字段由 CLI 投影同进同出（规格 4.2）：缺任一、分母为零、或分子非正时一律未知——
 * 旧 CLI 不下发这两个字段，此时**不回退**到本机估算，同一标签下静默换口径比没有读数更糟。
 */
function readDecodeSpeed(row: TurnHeaderRow): number | null {
  const { decodeMs, decodeTokens } = row;
  if (decodeMs === undefined || decodeTokens === undefined) return null;
  if (!Number.isFinite(decodeMs) || decodeMs <= 0) return null;
  if (!Number.isFinite(decodeTokens) || decodeTokens <= 0) return null;
  const value = decodeTokens / (decodeMs / 1_000);
  return Number.isFinite(value) && value > 0 ? value : null;
}

function createRecord(sessionId: string): SpeedRecord {
  return {
    sessionId,
    samples: [],
    rowKindByRowId: new Map(),
    turnRowId: null,
    turnRunning: false,
    segmentStartedAt: null,
    settled: false,
    average: null,
    view: NO_GENERATION_SPEED,
    listeners: new Set(),
    timer: null,
    updatedAt: 0,
  };
}

export class LocalGenerationSpeedTracker {
  private readonly records = new Map<string, SpeedRecord>();

  constructor(private readonly now: () => number = localTtftNow) {}

  /**
   * 消费一条 conversation topic 帧。Desktop `desktop-continuous` 与手机 `web-remote-replayable`
   * 共用本入口，差异只由 `delivery` 表达。
   */
  receive(frame: ConversationTopicFrame, delivery: TopicFrameDeliveryKind): void {
    const sessionId = parseConversationTopic(frame.topic);
    if (!sessionId) return;
    const at = this.now();
    const record = this.ensure(sessionId);
    record.updatedAt = at;

    if (delivery === "initial") {
      // 订阅起点不是投递缺口：只做种子同步，不计内容（快照里的整段文本不是"刚生成的"）。
      if (frame.payload.kind === "snapshot") this.seed(record, frame.payload.snapshot, at);
      return;
    }
    if (delivery !== "online") {
      // recovery 会重放错过的 deltas；按"本机实时观测"口径它们不是刚刚生成的，一律排除。
      this.discardWindow(record, at);
      return;
    }
    if (frame.payload.kind === "snapshot") {
      // online snapshot = 桌面连续订阅在缓冲溢出/投影重建后用快照代替 deltas，无法逐条归因。
      this.discardWindow(record, at);
      return;
    }

    this.applyDeltas(record, frame, at);
    this.refresh(record, at);
    this.evict();
  }

  /** 订阅某会话的视图变化；返回退订函数。 */
  subscribe(sessionId: string, listener: () => void): () => void {
    const record = this.ensure(sessionId);
    record.listeners.add(listener);
    return () => {
      record.listeners.delete(listener);
    };
  }

  /** 读取已发布视图；未知会话返回稳定空视图。 */
  read(sessionId: string): GenerationSpeedView {
    return this.records.get(sessionId)?.view ?? NO_GENERATION_SPEED;
  }

  /** 时间到点后窗口会自然排空，必须靠定时刷新把读数降回占位符。 */
  private scheduleTick(record: SpeedRecord): void {
    if (record.timer !== null) return;
    if (record.settled || record.samples.length === 0) return;
    record.timer = setTimeout(() => {
      record.timer = null;
      this.refresh(record, this.now());
    }, GENERATION_SPEED_VIEW_TICK_MS);
  }

  private ensure(sessionId: string): SpeedRecord {
    const existing = this.records.get(sessionId);
    if (existing) return existing;
    const record = createRecord(sessionId);
    this.records.set(sessionId, record);
    return record;
  }

  /** 只淘汰既无订阅也不在生成的会话；有面板打开或正在生成时必须保住。 */
  private evict(): void {
    if (this.records.size <= GENERATION_SPEED_MAX_SESSIONS) return;
    const candidates = [...this.records.values()]
      .filter((record) => record.listeners.size === 0 && !record.turnRunning)
      .sort((left, right) => left.updatedAt - right.updatedAt);
    for (const record of candidates) {
      if (this.records.size <= GENERATION_SPEED_MAX_SESSIONS) break;
      if (record.timer !== null) clearTimeout(record.timer);
      this.records.delete(record.sessionId);
    }
  }

  private seed(record: SpeedRecord, snapshot: ConversationSnapshot, at: number): void {
    record.rowKindByRowId.clear();
    let latestTurn: TurnHeaderRow | undefined;
    for (const row of snapshot.rows.window) {
      const kind = COUNTED_ROW_KINDS[row.kind];
      if (kind) this.rememberRowKind(record, row.rowId, kind);
      if (row.kind === "turnHeader") latestTurn = row;
    }
    record.turnRowId = latestTurn?.rowId ?? null;
    record.turnRunning = latestTurn?.state === "running";
    // 订阅时的快照只做状态对账：快照里的整段文本不是"刚生成的"，窗口与保留值都从零开始。
    record.settled = false;
    record.average = null;
    record.samples = [];
    record.segmentStartedAt = null;
    this.refresh(record, at);
  }

  /**
   * 投递缺口（重连、投影 resync、订阅时已在生成）里重放的内容不是"刚刚生成的"，
   * 一律不计入窗口，并清空当前窗口与生成段（规格 5.1）。缺口只影响实时值：轮末解码速度
   * 由 CLI 投影累计，与客户端是否漏帧无关。
   */
  private discardWindow(record: SpeedRecord, at: number): void {
    record.samples = [];
    record.segmentStartedAt = null;
    this.refresh(record, at);
  }

  private applyDeltas(record: SpeedRecord, frame: ConversationTopicFrame, at: number): void {
    if (frame.payload.kind !== "deltas") return;
    for (const delta of frame.payload.deltas) {
      switch (delta.op) {
        case "row.appended":
        case "row.upserted":
          this.applyRow(record, delta.row);
          break;
        case "row.delta":
          this.applyStreamDelta(record, delta.rowId, delta.path, delta.append, at);
          break;
        case "row.removed":
          this.applyRowRemoved(record, delta.fromRowId);
          break;
        default:
          break;
      }
    }
  }

  private applyRow(record: SpeedRecord, row: ConversationRow): void {
    if (row.kind === "turnHeader") {
      this.applyTurnHeader(record, row);
      return;
    }
    const kind = COUNTED_ROW_KINDS[row.kind];
    if (!kind) return;
    this.rememberRowKind(record, row.rowId, kind);
  }

  private applyTurnHeader(record: SpeedRecord, row: TurnHeaderRow): void {
    if (row.state === "running") {
      // turnHeader 每轮一行；同一行会在轮内被反复 upsert（工时/文件摘要更新），
      // 只有新 rowId 才是新的一轮，不能把进度 upsert 当成重置。
      if (record.turnRowId !== row.rowId) this.startTurn(record, row.rowId);
      return;
    }
    if (record.settled) return;
    if (record.turnRowId !== null && record.turnRowId !== row.rowId) return;
    this.settle(record, row);
  }

  private startTurn(record: SpeedRecord, rowId: number): void {
    record.turnRowId = rowId;
    record.turnRunning = true;
    record.settled = false;
    record.samples = [];
    record.segmentStartedAt = null;
    // 规格 2.1：新轮开始清空上一轮保留值，回到占位符。
    record.average = null;
    // 行 ID 每轮新增，历史行不会在下一轮继续追加；保留登记表只会让增量停在旧轮。
    record.rowKindByRowId.clear();
  }

  /** 结算即读权威字段：本模块不参与轮末口径的推导（规格 4.2）。 */
  private settle(record: SpeedRecord, row: TurnHeaderRow): void {
    record.settled = true;
    record.turnRunning = false;
    record.samples = [];
    record.segmentStartedAt = null;
    record.average = readDecodeSpeed(row);
  }

  private rememberRowKind(record: SpeedRecord, rowId: number, kind: CountedRowKind): void {
    record.rowKindByRowId.delete(rowId);
    record.rowKindByRowId.set(rowId, kind);
    if (record.rowKindByRowId.size <= GENERATION_SPEED_MAX_TRACKED_ROWS) return;
    const oldest = record.rowKindByRowId.keys().next().value;
    if (oldest !== undefined) record.rowKindByRowId.delete(oldest);
  }

  private applyRowRemoved(record: SpeedRecord, fromRowId: number): void {
    for (const rowId of record.rowKindByRowId.keys())
      if (rowId >= fromRowId) record.rowKindByRowId.delete(rowId);
  }

  /**
   * 只有在线 `row.delta` 计入：`row.upserted` / `row.appended` 是状态对账、可以重复出现
   * （含 output-token Continue 重新打开同一 row），按整段文本计数会重复计入（规格 4.2）。
   */
  private applyStreamDelta(
    record: SpeedRecord,
    rowId: number,
    path: StreamablePath,
    append: string,
    at: number,
  ): void {
    if (!append) return;
    const kind = record.rowKindByRowId.get(rowId);
    if (!kind) return;
    if (kind === "toolCall" ? path !== "inputText" : path !== "text") return;
    const tokens = estimateTokens(append);
    if (tokens <= 0) return;
    record.samples.push({ at, tokens });
    if (record.segmentStartedAt === null) record.segmentStartedAt = at;
  }

  private prune(record: SpeedRecord, at: number): void {
    const cutoff = at - GENERATION_SPEED_WINDOW_MS;
    while (record.samples.length > 0 && record.samples[0]!.at <= cutoff) record.samples.shift();
  }

  private computeLive(record: SpeedRecord, at: number): number | null {
    this.prune(record, at);
    let tokens = 0;
    for (const sample of record.samples) tokens += sample.tokens;
    if (tokens <= 0) return null;
    // 起步阶段按已生成时长归一，避免不足 3 秒时被整窗稀释。
    const spanMs =
      record.segmentStartedAt === null
        ? GENERATION_SPEED_WINDOW_MS
        : Math.min(GENERATION_SPEED_WINDOW_MS, at - record.segmentStartedAt);
    if (spanMs <= 0) return null;
    const value = (tokens * 1_000) / spanMs;
    return Number.isFinite(value) && value > 0 ? value : null;
  }

  private computeView(record: SpeedRecord, at: number): GenerationSpeedView {
    if (record.settled)
      return record.average === null
        ? NO_GENERATION_SPEED
        : { mode: "average", value: record.average };
    const live = this.computeLive(record, at);
    return live === null ? NO_GENERATION_SPEED : { mode: "live", value: live };
  }

  private refresh(record: SpeedRecord, at: number): void {
    const next = this.computeView(record, at);
    if (!sameView(record.view, next)) {
      record.view = next;
      for (const listener of record.listeners) {
        try {
          listener();
        } catch {
          /* 观测失败不能阻断内容投影或结算。 */
        }
      }
    }
    this.scheduleTick(record);
  }
}

/** 视图只在语义变化时替换对象；比对按显示精度（一位小数）进行，避免每帧都触发重渲染。 */
function sameView(left: GenerationSpeedView, right: GenerationSpeedView): boolean {
  if (left.mode !== right.mode) return false;
  if (left.value === null || right.value === null) return left.value === right.value;
  return roundForDisplay(left.value) === roundForDisplay(right.value);
}

function roundForDisplay(value: number): number {
  return Math.round(value * 10) / 10;
}

let tracker: LocalGenerationSpeedTracker | undefined;

export function getLocalGenerationSpeedTracker(): LocalGenerationSpeedTracker {
  tracker ??= new LocalGenerationSpeedTracker();
  return tracker;
}

/** 测试与宿主卸载用；传 undefined 表示下次读取时重建。 */
export function setLocalGenerationSpeedTracker(
  value: LocalGenerationSpeedTracker | undefined,
): void {
  tracker = value;
}
