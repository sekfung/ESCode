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
 * 会话实时 token 生成速度的本地观测器。
 *
 * 为什么在 Renderer 而不是 Runtime：流式阶段协议不携带逐 chunk token 数，provider 只在每次模型请求
 * finish 时给 usage（见 `core/src/runtime/methods/model.ts`），任何一层的实时值都只能是按流出文本的估算。
 * 观测口径、缺口语义与显示规则见 `docs/specs/conversation-generation-speed.md` 与
 * `docs/adr/0006-generation-speed-observed-in-renderer.md`。
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
  /** 处于 `running` 状态的工具行；工具执行区间不计入本轮生成时间（规格 4.3）。 */
  runningToolRows: Set<number>;
  /** 快照/补丁里最后一次可见的待交互数量（授权、用户回答、Hook 审核）。 */
  pendingInteractions: number;
  /** 当前 product turn 的 turnHeader rowId；进度 upsert 与新一轮靠它区分。 */
  turnRowId: number | null;
  turnRunning: boolean;
  /** 本轮首个 delta 的本地时间；用于起步阶段归一，缺口后重置为 null。 */
  segmentStartedAt: number | null;
  /** 上次结算时间的锚点；缺口帧把它前移以排除未知区间。 */
  lastAccountedAt: number | null;
  /** 本轮累计生成时间（毫秒），已排除工具执行与等待交互区间。 */
  activeMs: number;
  /** 本轮估算 token 累计。 */
  estimatedTokens: number;
  /** 本轮是否出现过投递缺口（重连、投影 resync、订阅时已在生成）。 */
  gap: boolean;
  settled: boolean;
  average: number | null;
  /** 结算请求：本帧观察到回合结束，等本次时间结算完成后再提交平均值。 */
  settleRequested: boolean;
  settleExactTokens: number | null;
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

function createRecord(sessionId: string): SpeedRecord {
  return {
    sessionId,
    samples: [],
    rowKindByRowId: new Map(),
    runningToolRows: new Set(),
    pendingInteractions: 0,
    turnRowId: null,
    turnRunning: false,
    segmentStartedAt: null,
    lastAccountedAt: null,
    activeMs: 0,
    estimatedTokens: 0,
    gap: false,
    settled: false,
    average: null,
    settleRequested: false,
    settleExactTokens: null,
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
      this.markGap(record, at);
      return;
    }
    if (frame.payload.kind === "snapshot") {
      // online snapshot = 桌面连续订阅在缓冲溢出/投影重建后用快照代替 deltas，无法逐条归因。
      this.markGap(record, at);
      return;
    }

    // 先取本帧之前的"阻塞"状态：区间 [上一帧, 本帧] 的归属由起点状态决定，
    // 反过来会把工具执行的整段时间算进分母（工具置 running 的那帧之后、结束那帧之前）。
    const blockedBefore = this.blocked(record);
    const counted = this.applyDeltas(record, frame, at);
    this.advance(record, at, blockedBefore, counted);
    if (record.settleRequested) this.commitSettle(record);
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

  /**
   * 工具执行与等待授权/用户回答期间模型没有生成，不能进分母。
   *
   * 注意这是区间起点的状态而不是"本帧是否在跑工具"：后台工具与模型并行生成时，区间会因为
   * 末尾带着 counted delta 而被计入，不会把并行生成的时间错误地整段排除。
   */
  private blocked(record: SpeedRecord): boolean {
    return record.pendingInteractions > 0 || record.runningToolRows.size > 0;
  }

  private advance(record: SpeedRecord, at: number, blockedBefore: boolean, counted: boolean): void {
    if (record.lastAccountedAt === null) return;
    if (record.turnRunning && !record.settled && (!blockedBefore || counted))
      record.activeMs += Math.max(0, at - record.lastAccountedAt);
    record.lastAccountedAt = at;
  }

  private seed(record: SpeedRecord, snapshot: ConversationSnapshot, at: number): void {
    record.pendingInteractions = snapshot.pendingInteractions.length;
    record.runningToolRows.clear();
    record.rowKindByRowId.clear();
    let latestTurn: TurnHeaderRow | undefined;
    for (const row of snapshot.rows.window) {
      const kind = COUNTED_ROW_KINDS[row.kind];
      if (kind) this.rememberRowKind(record, row.rowId, kind);
      if (row.kind === "toolCall" && row.status === "running" && row.backgrounded !== true)
        record.runningToolRows.add(row.rowId);
      if (row.kind === "turnHeader") latestTurn = row;
    }
    record.turnRowId = latestTurn?.rowId ?? null;
    record.turnRunning = latestTurn?.state === "running";
    record.settled = false;
    record.average = null;
    record.samples = [];
    record.segmentStartedAt = null;
    record.lastAccountedAt = null;
    record.activeMs = 0;
    record.estimatedTokens = 0;
    if (record.turnRunning) {
      // 订阅时这一轮已经在生成：本轮只剩尾巴，不能给出一个"看起来完整"的平均值。
      record.gap = true;
      record.lastAccountedAt = at;
    } else {
      record.gap = false;
      record.settleRequested = false;
    }
    this.refresh(record, at);
  }

  private markGap(record: SpeedRecord, at: number): void {
    record.gap = true;
    // 缺口覆盖了多少生成时间无从得知，不能计入分母；窗口也从头累积（规格 5）。
    record.samples = [];
    record.segmentStartedAt = null;
    if (record.lastAccountedAt !== null) record.lastAccountedAt = at;
    this.refresh(record, at);
  }

  private applyDeltas(record: SpeedRecord, frame: ConversationTopicFrame, at: number): boolean {
    if (frame.payload.kind !== "deltas") return false;
    let counted = false;
    for (const delta of frame.payload.deltas) {
      switch (delta.op) {
        case "row.appended":
        case "row.upserted":
          this.applyRow(record, delta.row);
          break;
        case "row.delta":
          if (this.applyStreamDelta(record, delta.rowId, delta.path, delta.append, at))
            counted = true;
          break;
        case "row.removed":
          this.applyRowRemoved(record, delta.fromRowId);
          break;
        case "state.updated":
          if (delta.patch.pendingInteractions !== undefined)
            record.pendingInteractions = delta.patch.pendingInteractions.length;
          break;
        default:
          break;
      }
    }
    return counted;
  }

  private applyRow(record: SpeedRecord, row: ConversationRow): void {
    if (row.kind === "turnHeader") {
      this.applyTurnHeader(record, row);
      return;
    }
    const kind = COUNTED_ROW_KINDS[row.kind];
    if (!kind) return;
    this.rememberRowKind(record, row.rowId, kind);
    if (row.kind !== "toolCall") return;
    // 只有主链路正在等待的工具执行才算"阻塞"：后台化的工具与模型并行，不能据此排除生成时间，
    // 否则一个长跑的后台 Bash 会把整轮分母抹掉。
    if (row.status === "running" && row.backgrounded !== true)
      record.runningToolRows.add(row.rowId);
    else record.runningToolRows.delete(row.rowId);
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
    // 结算推迟到本帧的时间结算之后：最后一个取值区间仍属于本轮。
    record.settleRequested = true;
    record.settleExactTokens = row.outputTokens ?? null;
  }

  private startTurn(record: SpeedRecord, rowId: number): void {
    record.turnRowId = rowId;
    record.turnRunning = true;
    record.settled = false;
    record.gap = false;
    record.samples = [];
    record.segmentStartedAt = null;
    record.lastAccountedAt = null;
    record.activeMs = 0;
    record.estimatedTokens = 0;
    // 规格 2.1：新轮开始清空上一轮保留值，回到占位符。
    record.average = null;
    record.settleRequested = false;
    record.settleExactTokens = null;
    // 行 ID 每轮新增，历史行不会在下一轮继续追加；保留登记表只会让增量停在旧轮。
    record.rowKindByRowId.clear();
  }

  private commitSettle(record: SpeedRecord): void {
    record.settleRequested = false;
    record.settled = true;
    record.turnRunning = false;
    record.samples = [];
    record.segmentStartedAt = null;
    const seconds = record.activeMs / 1_000;
    // 规格 4.3：有缺口不给平均值；分子优先用服务端 output 计量（规格 4.2 的轮末修正）。
    const numerator = record.gap ? null : (record.settleExactTokens ?? record.estimatedTokens);
    const average = numerator !== null && numerator > 0 && seconds > 0 ? numerator / seconds : null;
    record.average = Number.isFinite(average) ? average : null;
    record.settleExactTokens = null;
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
    for (const rowId of record.runningToolRows)
      if (rowId >= fromRowId) record.runningToolRows.delete(rowId);
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
  ): boolean {
    if (!append) return false;
    const kind = record.rowKindByRowId.get(rowId);
    if (!kind) return false;
    if (kind === "toolCall" ? path !== "inputText" : path !== "text") return false;
    const tokens = estimateTokens(append);
    if (tokens <= 0) return false;
    record.samples.push({ at, tokens });
    record.estimatedTokens += tokens;
    if (record.segmentStartedAt === null) {
      record.segmentStartedAt = at;
      record.lastAccountedAt = at;
    }
    return true;
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
