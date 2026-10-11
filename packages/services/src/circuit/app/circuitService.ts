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
import { writeCircuitSnapshot } from "./circuitExport.js";
import { CircuitWriteQueue } from "./circuitWriteQueue.js";
import {
  circuitIrIdOf,
  circuitLocationKey as keyOf,
  circuitLocationOf as locationOf,
  type CircuitLocation,
} from "./circuitLocation.js";
import { detectCircuitSourceKind, extractCircuitSource } from "./sourceExtraction.js";
import {
  dropOrphanedCorrection,
  requireCircuitRevision,
} from "./correctionMaintenance.js";
import type { CircuitServicePorts, CircuitSourceWatch, StoredCircuit } from "./ports.js";

/**
 * CircuitService：Circuit IR 的唯一所有者（ADR 0005）。
 * 写入路径与事件顺序（§3.3、§3.4）、规则检查与固件比对的只读边界，都见 CONTRACT.md。
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
    const irId = stored?.ir.irId ?? circuitIrIdOf(location, this.options.ports.source.hashContent);
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
    const watches = await this.watchSourceFiles(location, record, sourceFiles);
    const committed: CircuitState = { location, record, watches };
    this.stateByKey.set(keyOf(location), committed);
    this.keyByIrId.set(record.irId, keyOf(location));
    return committed;
  }

  private async watchSourceFiles(
    location: CircuitLocation,
    record: StoredCircuit,
    sourceFiles: string[],
  ): Promise<CircuitSourceWatch[]> {
    const watches: CircuitSourceWatch[] = [];
    for (const relativePath of sourceFiles) {
      try {
        watches.push(
          await this.options.ports.watcher.watch({
            workspacePath: location.workspacePath,
            relativePath,
            onChange: () => void this.markStale(location),
          }),
        );
      } catch (error) {
        // 监听失败不阻塞解析：IR 仍可用，只是不会自动标 stale。
        log.warn("circuit source watch failed", {
          irId: record.irId,
          path: relativePath,
          message: error instanceof Error ? error.message : String(error),
        });
      }
    }
    return watches;
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
