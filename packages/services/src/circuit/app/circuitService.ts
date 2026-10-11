import { Emitter } from "@escode/rpc";
import type {
  CircuitChangedEvent,
  CircuitCheckParams,
  CircuitCheckResult,
  CircuitCorrectParams,
  CircuitCorrectionListEntry,
  CircuitExportParams,
  CircuitExportResult,
  CircuitFindBySourceParams,
  CircuitFirmwareCompareParams,
  CircuitFirmwareCompareResult,
  CircuitIrParams,
  CircuitIrView,
  CircuitLayout,
  CircuitParseParams,
  CircuitQueryParams,
  CircuitQueryResult,
  CircuitSubmitRecognitionParams,
  ICircuitService,
} from "../contract.js";
import { createServiceLogger } from "../../logger/serviceLogger.js";
import { applyCircuitCorrections } from "../domain/circuitCorrections.js";
import { CircuitServiceError } from "../domain/circuitErrors.js";
import { revalidateCircuitIr } from "../domain/irBuild.js";
import { circuitIrViewOf } from "./irView.js";
import { runCircuitQuery } from "./circuitQuery.js";
import { runCircuitCheck } from "./circuitCheck.js";
import { runCircuitFirmwareCompare } from "./circuitFirmwareCompare.js";
import { writeCircuitSnapshot } from "./circuitExport.js";
import { planCircuitParse } from "./circuitParse.js";
import { planCircuitRecognition } from "./circuitRecognition.js";
import { CircuitWriteQueue } from "./circuitWriteQueue.js";
import {
  circuitLocationKey as keyOf,
  circuitLocationOf as locationOf,
  indexCircuitIrId,
  type CircuitLocation,
} from "./circuitLocation.js";
import { watchCircuitSourceFiles } from "./sourceWatching.js";
import {
  dropOrphanedCorrection,
  requireCircuitRevision,
  requireReparseRevision,
} from "./correctionMaintenance.js";
import type { CircuitServicePorts, CircuitSourceWatch, StoredCircuit } from "./ports.js";

/**
 * CircuitService：Circuit IR 的唯一所有者（ADR 0005）。写入路径与事件顺序（§3.3、§3.4）、
 * 规则检查 / 固件比对 / 识别的只读与写入边界，都见 CONTRACT.md。
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
  private readonly writes = new CircuitWriteQueue();
  private readonly now: () => string;

  constructor(private readonly options: CreateCircuitServiceOptions) {
    this.now = options.now ?? (() => new Date().toISOString());
  }

  async parse(params: CircuitParseParams): Promise<CircuitIrView> {
    const location = locationOf(params);
    return this.writes.run(keyOf(location), () => this.parseInternal(location, params));
  }

  /**
   * 提交识别结果（§5）：与 parse 同一条写入路径与事件顺序——
   * 校验/重映射在 planCircuitRecognition 里完成，持久化（store 写入 + 源文件监听）走 commit，
   * 事件在持久化之后发出（§3.3）。
   */
  async submitRecognition(params: CircuitSubmitRecognitionParams): Promise<CircuitIrView> {
    const location = locationOf(params);
    return this.writes.run(keyOf(location), async () => {
      const state = await this.stateFor(location);
      const plan = await planCircuitRecognition({
        location,
        params,
        ...(state ? { stored: state.record } : {}),
        source: this.options.ports.source,
        vector: this.options.ports.vector,
      });
      const committed = await this.commit(state, location, plan.record, [location.sourcePath]);
      this.changed.fire({
        irId: committed.record.irId,
        revision: committed.record.ir.revision,
        stale: false,
      });
      return circuitIrViewOf(committed.record.ir);
    });
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
    return runCircuitCheck(params, this.options.ports.source, async () => (await this.recordFor(params)).ir);
  }

  compareFirmware(params: CircuitFirmwareCompareParams): Promise<CircuitFirmwareCompareResult> {
    return runCircuitFirmwareCompare(
      params,
      this.options.ports.source,
      async () => (await this.recordFor(params)).ir,
    );
  }

  async correct(params: CircuitCorrectParams): Promise<CircuitIrView> {
    const location = await this.locationFor(params);
    return this.writes.run(keyOf(location), async () => {
      const state = await this.stateFor(location);
      if (!state) throw new CircuitServiceError("not_found", `IR ${params.irId} 不存在`);
      const record = state.record;
      requireCircuitRevision(record, params.baseRevision);
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

  /** 导出只读快照（§2 规则 8）：委托给 app/circuitExport，服务只提供当前记录与写入端口。 */
  async exportSnapshot(params: CircuitExportParams): Promise<CircuitExportResult> {
    const location = await this.locationFor(params);
    const state = await this.stateFor(location);
    if (!state) throw new CircuitServiceError("not_found", `IR ${params.irId} 不存在`);
    const result = await writeCircuitSnapshot({
      location,
      record: state.record,
      params,
      snapshot: this.options.ports.snapshot,
      now: this.now,
    });
    log.info("circuit export", { irId: state.record.irId, revision: state.record.ir.revision });
    return result;
  }

  async dropCorrection(
    params: CircuitIrParams & { baseRevision: number; index: number },
  ): Promise<CircuitIrView> {
    const location = await this.locationFor(params);
    return this.writes.run(keyOf(location), async () => {
      const state = await this.stateFor(location);
      if (!state) throw new CircuitServiceError("not_found", `IR ${params.irId} 不存在`);
      const record = state.record;
      requireCircuitRevision(record, params.baseRevision);
      const next = dropOrphanedCorrection(record, params.index);
      await this.persist(state, next);
      log.info("circuit drop orphaned correction", { irId: next.irId, index: params.index });
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
    if (stored) requireReparseRevision(stored, params.baseRevision);
    const plan = await planCircuitParse({
      location,
      params,
      ...(stored ? { stored } : {}),
      source: this.options.ports.source,
    });
    const committed = await this.commit(state, location, plan.record, plan.sourceFiles);
    const record = committed.record;
    log.info("circuit parse", {
      irId: record.irId,
      kind: record.ir.source.kind,
      revision: record.ir.revision,
      components: record.ir.components.length,
      nets: record.ir.nets.length,
      diagnostics: record.ir.diagnostics.length,
      orphaned: record.corrections.filter((entry) => entry.orphaned).length,
      ms: Date.now() - started,
    });
    // 持久化完成之后才发事件（§3.3）。
    this.changed.fire({ irId: record.irId, revision: record.ir.revision, stale: false });
    return circuitIrViewOf(record.ir);
  }

  /** 源文件变化：只翻转 stale，revision 不变（§3.4）。已在 stale 状态时不再重复发事件。 */
  private async markStale(location: CircuitLocation): Promise<void> {
    await this.writes.run(keyOf(location), async () => {
      const state = await this.stateFor(location);
      if (!state || state.record.ir.stale) return;
      const record = state.record;
      const next: StoredCircuit = { ...record, ir: { ...record.ir, stale: true } };
      await this.persist(state, next);
      log.info("circuit source stale", { irId: record.irId, revision: record.ir.revision });
      this.changed.fire({ irId: record.irId, revision: record.ir.revision, stale: true });
    });
  }

  /** store 写入只有这一处（§3.2）：持久化先于内存与事件，任何写入路径都经这里。 */
  private async writeStore(location: CircuitLocation, record: StoredCircuit): Promise<void> {
    await this.options.ports.store.write({
      identityKey: location.identityKey,
      sourcePath: location.sourcePath,
      record,
    });
  }

  private async persist(state: CircuitState, record: StoredCircuit): Promise<void> {
    await this.writeStore(state.location, record);
    this.stateByKey.set(keyOf(state.location), { ...state, record });
    indexCircuitIrId(this.keyByIrId, state.location, record.irId, state.record.irId);
  }

  /** 首次解析 / 首次识别：持久化 → 更新内存 → 监听这次真正读到的源文件（含子图纸）。 */
  private async commit(
    state: CircuitState | undefined,
    location: CircuitLocation,
    record: StoredCircuit,
    sourceFiles: string[],
  ): Promise<CircuitState> {
    await this.writeStore(location, record);
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
    indexCircuitIrId(this.keyByIrId, location, record.irId, state?.record.irId);
    return committed;
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
}

export function createCircuitService(options: CreateCircuitServiceOptions): CircuitService {
  return new CircuitServiceImpl(options);
}
