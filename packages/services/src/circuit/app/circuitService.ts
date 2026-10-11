import { Emitter } from "@escode/rpc";
import type { CircuitIR } from "@escode/shared/circuit";
import type {
  CircuitChangedEvent,
  CircuitCheckParams,
  CircuitCheckResult,
  CircuitCorrectParams,
  CircuitCorrectionListEntry,
  CircuitFindBySourceParams,
  CircuitFirmwareCompareParams,
  CircuitFirmwareCompareResult,
  CircuitIrParams,
  CircuitIrView,
  CircuitLayout,
  CircuitParseParams,
  CircuitQueryParams,
  CircuitQueryResult,
  ICircuitService,
} from "../contract.js";
import { createServiceLogger } from "../../logger/serviceLogger.js";
import { applyCircuitCorrections, reapplyCircuitCorrections } from "../domain/circuitCorrections.js";
import { CircuitServiceError } from "../domain/circuitErrors.js";
import { assembleCircuitIr, revalidateCircuitIr } from "../domain/irBuild.js";
import { circuitIrViewOf } from "./irView.js";
import { runCircuitQuery } from "./circuitQuery.js";
import { runCircuitCheck } from "./circuitCheck.js";
import { runCircuitFirmwareCompare } from "./circuitFirmwareCompare.js";
import { watchCircuitSourceFiles } from "./sourceWatching.js";
import {
  circuitLocationKey as keyOf,
  circuitLocationOf as locationOf,
  type CircuitLocation,
} from "./circuitLocation.js";
import { detectCircuitSourceKind, extractCircuitSource } from "./sourceExtraction.js";
import type { CircuitServicePorts, CircuitSourceWatch, StoredCircuit } from "./ports.js";

/**
 * CircuitService：Circuit IR 的唯一所有者（ADR 0005）。
 * 写入路径、事件顺序（§3.3、§3.4）与规则检查 / 固件比对的只读边界见 CONTRACT.md。
 */

const log = createServiceLogger("circuit");

export interface CreateCircuitServiceOptions {
  ports: CircuitServicePorts;
  /** 便于测试注入时间源。 */
  now?: () => string;
}

export interface CircuitService extends ICircuitService {
  /** 释放所有源文件监听（窗口 Host 销毁时调用，同步）。 */
  dispose(): void;
}

interface CircuitState {
  location: CircuitLocation;
  record: StoredCircuit;
  watches: CircuitSourceWatch[];
}

class CircuitServiceImpl implements CircuitService {
  private readonly changed = new Emitter<CircuitChangedEvent>();
  readonly onChanged = this.changed.event;
  private readonly stateByKey = new Map<string, CircuitState>();
  private readonly keyByIrId = new Map<string, string>();
  private readonly queues = new Map<string, Promise<unknown>>();
  private readonly now: () => string;

  constructor(private readonly options: CreateCircuitServiceOptions) {
    this.now = options.now ?? (() => new Date().toISOString());
  }

  async parse(params: CircuitParseParams): Promise<CircuitIrView> {
    const location = locationOf(params);
    return this.enqueue(location, () => this.parseInternal(location, params));
  }

  async findBySource(
    params: CircuitFindBySourceParams,
  ): Promise<{ irId: string; revision: number; stale: boolean } | undefined> {
    const state = await this.stateFor(locationOf(params));
    if (!state) return undefined;
    return {
      irId: state.record.irId,
      revision: state.record.ir.revision,
      stale: state.record.ir.stale,
    };
  }

  async get(params: CircuitIrParams): Promise<CircuitIrView> {
    return circuitIrViewOf((await this.recordFor(params)).ir);
  }

  async getLayout(params: CircuitIrParams): Promise<CircuitLayout | undefined> {
    return (await this.recordFor(params)).layout;
  }

  async query(params: CircuitQueryParams): Promise<CircuitQueryResult> {
    return runCircuitQuery((await this.recordFor(params)).ir, params);
  }

  check(params: CircuitCheckParams): Promise<CircuitCheckResult> {
    return runCircuitCheck(params, this.options.ports.source, () => this.irFor(params));
  }

  compareFirmware(params: CircuitFirmwareCompareParams): Promise<CircuitFirmwareCompareResult> {
    return runCircuitFirmwareCompare(params, this.options.ports.source, () => this.irFor(params));
  }

  async correct(params: CircuitCorrectParams): Promise<CircuitIrView> {
    const location = await this.locationFor(params);
    return this.enqueue(location, async () => {
      const state = await this.stateFor(location);
      if (!state) throw new CircuitServiceError("not_found", `IR ${params.irId} 不存在`);
      const record = state.record;
      if (params.baseRevision !== record.ir.revision)
        throw new CircuitServiceError(
          "stale_revision",
          `当前 revision 为 ${record.ir.revision}，收到 baseRevision ${params.baseRevision}；请重读后再写入`,
        );
      const { ir: corrected, applied } = applyCircuitCorrections(
        record.ir,
        params.ops,
        params.author,
      );
      const revision = corrected.revision + 1;
      const ir = revalidateCircuitIr({ ...corrected, revision });
      const next: StoredCircuit = {
        irId: record.irId,
        ir,
        ...(record.layout ? { layout: { ...record.layout, revision } } : {}),
        corrections: [
          ...record.corrections,
          ...applied.map(({ op, target }) => ({
            revision,
            appliedAt: this.now(),
            author: params.author,
            target,
            op,
          })),
        ],
      };
      await this.persist(state, next);
      log.info("circuit correct", {
        irId: record.irId,
        revision,
        ops: params.ops.length,
        author: params.author.kind,
      });
      this.changed.fire({ irId: record.irId, revision, stale: ir.stale });
      return circuitIrViewOf(ir);
    });
  }

  async listCorrections(params: CircuitIrParams): Promise<CircuitCorrectionListEntry[]> {
    const record = await this.recordFor(params);
    return record.corrections.map((entry, index) => ({ index, record: entry }));
  }

  async dropCorrection(
    params: CircuitIrParams & { baseRevision: number; index: number },
  ): Promise<CircuitIrView> {
    const location = await this.locationFor(params);
    return this.enqueue(location, async () => {
      const state = await this.stateFor(location);
      if (!state) throw new CircuitServiceError("not_found", `IR ${params.irId} 不存在`);
      const record = state.record;
      if (params.baseRevision !== record.ir.revision)
        throw new CircuitServiceError(
          "stale_revision",
          `当前 revision 为 ${record.ir.revision}，收到 baseRevision ${params.baseRevision}`,
        );
      const entry = record.corrections[params.index];
      if (!entry) throw new CircuitServiceError("invalid_ir", `修正记录下标 ${params.index} 不存在`);
      if (!entry.orphaned)
        throw new CircuitServiceError("invalid_ir", "只能删除已失效（orphaned）的修正记录");
      const next: StoredCircuit = {
        ...record,
        corrections: record.corrections.filter((_, index) => index !== params.index),
      };
      await this.persist(state, next);
      log.info("circuit drop orphaned correction", { irId: record.irId, index: params.index });
      return circuitIrViewOf(next.ir);
    });
  }

  dispose(): void {
    for (const state of this.stateByKey.values())
      for (const watch of state.watches) watch.dispose();
    this.stateByKey.clear();
    this.keyByIrId.clear();
    this.changed.dispose();
  }

  private async parseInternal(
    location: CircuitLocation,
    params: CircuitParseParams,
  ): Promise<CircuitIrView> {
    const started = Date.now();
    const state = await this.stateFor(location);
    const stored = state?.record;
    if (stored) {
      if (params.baseRevision === undefined)
        throw new CircuitServiceError(
          "invalid_ir",
          "该源文件已有 IR，重新解析需要 baseRevision（先读再改）",
        );
      if (params.baseRevision !== stored.ir.revision)
        throw new CircuitServiceError(
          "stale_revision",
          `当前 revision 为 ${stored.ir.revision}，收到 baseRevision ${params.baseRevision}`,
        );
    }
    const kind = detectCircuitSourceKind(location.sourcePath);
    const text = await this.options.ports.source.readText({
      workspacePath: location.workspacePath,
      relativePath: location.sourcePath,
    });
    const contentHash = this.options.ports.source.hashContent(text);
    const irId = stored?.ir.irId ?? this.makeIrId(location);
    const revision = (stored?.ir.revision ?? 0) + 1;
    const sourceFiles = new Set<string>([location.sourcePath]);
    const { elements, layout } = await extractCircuitSource({
      kind,
      text,
      workspacePath: location.workspacePath,
      sourcePath: location.sourcePath,
      irId,
      revision,
      source: this.options.ports.source,
      sourceFiles,
    });
    const parsed = assembleCircuitIr({
      irId,
      revision,
      source: { kind, path: location.sourcePath, contentHash },
      stale: false,
      elements,
    });
    const reapplied = reapplyCircuitCorrections(parsed, stored?.corrections ?? []);
    const record: StoredCircuit = {
      irId,
      ir: reapplied.ir,
      ...(layout ? { layout: { ...layout, revision } } : {}),
      corrections: reapplied.records,
    };
    await this.commit(state, location, record, [...sourceFiles]);
    log.info("circuit parse", {
      irId,
      kind,
      revision,
      components: record.ir.components.length,
      nets: record.ir.nets.length,
      diagnostics: record.ir.diagnostics.length,
      orphaned: record.corrections.filter((entry) => entry.orphaned).length,
      ms: Date.now() - started,
    });
    // 持久化完成之后才发事件（§3.3）。
    this.changed.fire({ irId, revision, stale: false });
    return circuitIrViewOf(record.ir);
  }

  /** 源文件变化：只翻转 stale，revision 不变（§3.4）。已在 stale 状态时不再重复发事件。 */
  private async markStale(location: CircuitLocation): Promise<void> {
    await this.enqueue(location, async () => {
      const state = await this.stateFor(location);
      if (!state || state.record.ir.stale) return;
      const record = state.record;
      const next: StoredCircuit = { ...record, ir: { ...record.ir, stale: true } };
      await this.persist(state, next);
      log.info("circuit source stale", { irId: record.irId, revision: record.ir.revision });
      this.changed.fire({ irId: record.irId, revision: record.ir.revision, stale: true });
    });
  }

  private async persist(state: CircuitState, record: StoredCircuit): Promise<void> {
    await this.options.ports.store.write({
      identityKey: state.location.identityKey,
      sourcePath: state.location.sourcePath,
      record,
    });
    this.stateByKey.set(keyOf(state.location), { ...state, record });
    this.keyByIrId.set(record.irId, keyOf(state.location));
  }

  /** 首次解析：持久化 → 更新内存 → 监听这次真正读到的源文件（含子图纸）。 */
  private async commit(
    state: CircuitState | undefined,
    location: CircuitLocation,
    record: StoredCircuit,
    sourceFiles: string[],
  ): Promise<CircuitState> {
    await this.options.ports.store.write({
      identityKey: location.identityKey,
      sourcePath: location.sourcePath,
      record,
    });
    for (const watch of state?.watches ?? []) watch.dispose();
    const watches = await watchCircuitSourceFiles({
      watcher: this.options.ports.watcher,
      location,
      irId: record.irId,
      sourceFiles,
      onChange: () => void this.markStale(location),
    });
    const committed: CircuitState = { location, record, watches };
    this.stateByKey.set(keyOf(location), committed);
    this.keyByIrId.set(record.irId, keyOf(location));
    return committed;
  }

  private makeIrId(location: CircuitLocation): string {
    const digest = this.options.ports.source.hashContent(
      `${location.identityKey}\n${location.sourcePath}`,
    );
    return `ir:${digest.slice(0, 16)}`;
  }

  private async stateFor(
    location: CircuitLocation,
    record?: StoredCircuit,
  ): Promise<CircuitState | undefined> {
    const key = keyOf(location);
    const existing = this.stateByKey.get(key);
    if (existing && !record) return existing;
    const loaded =
      record ??
      (await this.options.ports.store.read({
        identityKey: location.identityKey,
        sourcePath: location.sourcePath,
      }));
    if (!loaded) return undefined;
    const state: CircuitState = { location, record: loaded, watches: existing?.watches ?? [] };
    this.stateByKey.set(key, state);
    this.keyByIrId.set(loaded.irId, key);
    return state;
  }

  /** irId → 记录位置：内存索引优先；Host 重启后按 irId 扫回该 workspace 的记录。 */
  private async locationFor(params: CircuitIrParams): Promise<CircuitLocation> {
    const key = this.keyByIrId.get(params.irId);
    const known = key ? this.stateByKey.get(key) : undefined;
    if (known) return known.location;
    const identityKey = params.workspaceIdentity?.trim() || params.workspacePath;
    const stored = await this.options.ports.store.findByIrId({ identityKey, irId: params.irId });
    if (!stored)
      throw new CircuitServiceError(
        "not_found",
        `IR ${params.irId} 不在当前 workspace 中（重新 parse 或 findBySource 可恢复）`,
      );
    const location: CircuitLocation = {
      workspacePath: params.workspacePath,
      ...(params.workspaceIdentity ? { workspaceIdentity: params.workspaceIdentity } : {}),
      ...(params.remoteSessionId ? { remoteSessionId: params.remoteSessionId } : {}),
      identityKey,
      sourcePath: stored.ir.source.path,
    };
    this.stateByKey.set(keyOf(location), { location, record: stored, watches: [] });
    this.keyByIrId.set(stored.irId, keyOf(location));
    return location;
  }

  private async recordFor(params: CircuitIrParams): Promise<StoredCircuit> {
    const location = await this.locationFor(params);
    const state = await this.stateFor(location);
    if (!state) throw new CircuitServiceError("not_found", `IR ${params.irId} 不存在`);
    return state.record;
  }

  /** 只读分析（check / compareFirmware）用：定位 IR 并取出当前 IR。 */
  private async irFor(params: CircuitIrParams): Promise<CircuitIR> {
    return (await this.recordFor(params)).ir;
  }

  /** 同一 sourcePath 的写入串行执行（§3.3）；前一个任务失败不影响后续任务。 */
  private enqueue<T>(location: CircuitLocation, task: () => Promise<T>): Promise<T> {
    const key = keyOf(location);
    const previous = this.queues.get(key) ?? Promise.resolve();
    const next = previous.then(task, task);
    this.queues.set(
      key,
      next.then(
        () => undefined,
        () => undefined,
      ),
    );
    return next;
  }
}

export function createCircuitService(options: CreateCircuitServiceOptions): CircuitService {
  return new CircuitServiceImpl(options);
}
