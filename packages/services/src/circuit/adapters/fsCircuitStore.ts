import { createHash } from "node:crypto";
import { mkdir, readFile, readdir, rm } from "node:fs/promises";
import { join } from "node:path";
import { circuitCorrectionOpSchema, circuitIrSchema, type CircuitIR } from "@escode/shared/circuit";
import { atomicWriteJson, atomicWriteText } from "../../fs/atomicFileUtils.js";
import { createServiceLogger, type ServiceLogger } from "../../logger/serviceLogger.js";
import { getAppConfigDir } from "../../paths.js";
import type { CircuitStorePort, StoredCircuit } from "../app/ports.js";
import type { CircuitLayout } from "../contract.js";
import type { CircuitCorrectionRecord } from "../domain/circuitCorrections.js";

/**
 * Circuit IR 的文件存储适配器（docs/circuit/README.md §3.2、§3.3，ADR 0005）。
 * 目录：`<rootDir>/<sha256(identityKey) 前 12 位>/<sha256(sourcePath) 前 12 位>/`，
 * 文件：`ir.json`（提交点）、`layout.json`（渲染几何）、`corrections.json`（修正记录）。
 * 日志只记录 key 哈希与计数，不写电路内容或文件内容（§9）。
 */

const HASH_LENGTH = 12;
const KEY_DIR_NAME = /^[a-f0-9]{12}$/u;

/** 与 paths.ts 的 getWorkspaceHash 保持同一命名约定：sha256 十六进制前 12 位。 */
function hashKey(value: string): string {
  return createHash("sha256").update(value).digest("hex").slice(0, HASH_LENGTH);
}

interface StorePaths {
  dir: string;
  irPath: string;
  layoutPath: string;
  correctionsPath: string;
}

/** 每个源文件一个目录：`<rootDir>/<hash12(identityKey)>/<hash12(sourcePath)>/`。 */
function storePathsInDir(dir: string): StorePaths {
  return {
    dir,
    irPath: join(dir, "ir.json"),
    layoutPath: join(dir, "layout.json"),
    correctionsPath: join(dir, "corrections.json"),
  };
}

function resolveStorePaths(rootDir: string, identityKey: string, sourcePath: string): StorePaths {
  return storePathsInDir(join(rootDir, hashKey(identityKey), hashKey(sourcePath)));
}

function errorCodeOf(error: unknown): string | undefined {
  if (typeof error !== "object" || error === null || !("code" in error)) {
    return undefined;
  }
  const code = (error as { code?: unknown }).code;
  return typeof code === "string" ? code : undefined;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function describeError(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** 读取文件；不存在（ENOENT）返回 undefined，其它 IO 失败抛 `io_error`。 */
async function readTextIfExists(filePath: string, fileName: string): Promise<string | undefined> {
  try {
    return await readFile(filePath, "utf-8");
  } catch (error) {
    if (errorCodeOf(error) === "ENOENT") {
      return undefined;
    }
    throw new Error(`io_error: 读取 ${fileName} 失败`);
  }
}

function parseJson(text: string, fileName: string): unknown {
  try {
    return JSON.parse(text) as unknown;
  } catch {
    throw new Error(`invalid_ir: ${fileName} 不是合法 JSON`);
  }
}

/** ir.json 必须同时满足 JSON 可解析与 circuitIrSchema 校验。 */
function parseIr(text: string): CircuitIR {
  const parsed = parseJson(text, "ir.json");
  const result = circuitIrSchema.safeParse(parsed);
  if (!result.success) {
    const first = result.error.issues[0];
    const detail = first ? `${first.path.join(".") || "<root>"} ${first.message}` : "结构不合法";
    throw new Error(`invalid_ir: ir.json 校验失败（${detail}）`);
  }
  return result.data;
}

function isCorrectionRecord(value: unknown): value is CircuitCorrectionRecord {
  if (!isRecord(value)) return false;
  const author = value.author;
  const target = value.target;
  return (
    typeof value.revision === "number" &&
    Number.isInteger(value.revision) &&
    value.revision >= 0 &&
    typeof value.appliedAt === "string" &&
    value.appliedAt.length > 0 &&
    isRecord(author) &&
    (author.kind === "user" || author.kind === "agent") &&
    (author.sessionId === undefined || typeof author.sessionId === "string") &&
    isRecord(target) &&
    (target.kind === "element" || target.kind === "net" || target.kind === "block") &&
    typeof target.id === "string" &&
    target.id.length > 0 &&
    (target.pin === undefined || typeof target.pin === "string") &&
    (value.orphaned === undefined || typeof value.orphaned === "boolean") &&
    circuitCorrectionOpSchema.safeParse(value.op).success
  );
}

function parseCorrections(text: string): CircuitCorrectionRecord[] {
  const parsed = parseJson(text, "corrections.json");
  if (!Array.isArray(parsed) || !parsed.every(isCorrectionRecord)) {
    throw new Error("invalid_ir: corrections.json 结构不合法");
  }
  return parsed;
}

/** layout.json 只做顶层结构校验：完整几何由解析侧保证，存储层只负责拒绝明显损坏的文件。 */
function isLayoutShape(value: unknown): value is CircuitLayout {
  return (
    isRecord(value) &&
    typeof value.irId === "string" &&
    value.irId.length > 0 &&
    typeof value.revision === "number" &&
    Number.isInteger(value.revision) &&
    Array.isArray(value.sheets)
  );
}

/**
 * 读取 layout.json。提交点规则：`ir.json` 是唯一提交点，layout 只在与 IR 的
 * irId / revision 完全对齐时才返回；否则说明最后一次写入中断或源文件已换成新 IR，
 * 此时旧几何必须忽略，不能交给渲染层。
 */
async function readLayout(
  layoutPath: string,
  ir: CircuitIR,
  log: ServiceLogger,
  keyHash: string,
): Promise<CircuitLayout | undefined> {
  const text = await readTextIfExists(layoutPath, "layout.json");
  if (text === undefined) return undefined;
  const parsed = parseJson(text, "layout.json");
  if (!isLayoutShape(parsed)) {
    throw new Error("invalid_ir: layout.json 结构不合法");
  }
  if (parsed.irId !== ir.irId || parsed.revision !== ir.revision) {
    log.warn(undefined, "电路布局与 IR 不对齐，忽略 layout", {
      key: keyHash,
      layoutRevision: parsed.revision,
      irRevision: ir.revision,
    });
    return undefined;
  }
  return parsed;
}

/** 组装一份完整记录：corrections 缺失视为空，layout 按提交点规则对齐后才返回。 */
async function assembleRecord(
  paths: StorePaths,
  ir: CircuitIR,
  log: ServiceLogger,
  keyHash: string,
): Promise<StoredCircuit> {
  const correctionsText = await readTextIfExists(paths.correctionsPath, "corrections.json");
  const corrections = correctionsText === undefined ? [] : parseCorrections(correctionsText);
  const layout = await readLayout(paths.layoutPath, ir, log, keyHash);
  return { irId: ir.irId, ir, ...(layout ? { layout } : {}), corrections };
}

export function createFsCircuitStore(options?: { rootDir?: string }): CircuitStorePort {
  const rootDir = options?.rootDir ?? join(getAppConfigDir(), "circuits");
  const log = createServiceLogger("circuit");

  return {
    async read({
      identityKey,
      sourcePath,
    }: {
      identityKey: string;
      sourcePath: string;
    }): Promise<StoredCircuit | undefined> {
      const paths = resolveStorePaths(rootDir, identityKey, sourcePath);
      const keyHash = hashKey(identityKey);
      // 目录或 ir.json 不存在都表示「还没有 IR」，不是错误。
      const irText = await readTextIfExists(paths.irPath, "ir.json");
      if (irText === undefined) {
        log.debug(undefined, "电路 IR 不存在", { key: keyHash });
        return undefined;
      }

      const record = await assembleRecord(paths, parseIr(irText), log, keyHash);
      log.debug(undefined, "读取电路 IR", {
        key: keyHash,
        revision: record.ir.revision,
        corrections: record.corrections.length,
        hasLayout: record.layout !== undefined,
      });
      return record;
    },

    /**
     * 按 irId 反查：irId 由 (identityKey, sourcePath) 派生、无法反解，只有扫目录逐份比对。
     * 扫描阶段单个目录的 ir.json 读失败或损坏只跳过并告警——一份损坏记录不能让其它记录查不出来；
     * 命中后按 `read` 的口径组装，corrections / layout 损坏照旧抛 `invalid_ir`。
     */
    async findByIrId({
      identityKey,
      irId,
    }: {
      identityKey: string;
      irId: string;
    }): Promise<StoredCircuit | undefined> {
      const keyHash = hashKey(identityKey);
      const keyDir = join(rootDir, keyHash);
      let entries;
      try {
        entries = await readdir(keyDir, { withFileTypes: true });
      } catch (error) {
        if (errorCodeOf(error) === "ENOENT") {
          return undefined;
        }
        throw new Error("io_error: 读取电路 IR 目录失败");
      }

      for (const entry of entries) {
        if (!entry.isDirectory() || !KEY_DIR_NAME.test(entry.name)) continue;
        const paths = storePathsInDir(join(keyDir, entry.name));
        let ir: CircuitIR;
        try {
          const irText = await readTextIfExists(paths.irPath, "ir.json");
          if (irText === undefined) continue;
          ir = parseIr(irText);
        } catch (error) {
          log.warn(undefined, "跳过无法读取的电路 IR 目录", {
            key: keyHash,
            dir: entry.name,
            error: describeError(error),
          });
          continue;
        }
        if (ir.irId !== irId) continue;

        const record = await assembleRecord(paths, ir, log, keyHash);
        log.debug(undefined, "按 irId 找到电路 IR", {
          key: keyHash,
          dir: entry.name,
          revision: record.ir.revision,
        });
        return record;
      }

      log.debug(undefined, "按 irId 未找到电路 IR", { key: keyHash });
      return undefined;
    },

    async write({
      identityKey,
      sourcePath,
      record,
    }: {
      identityKey: string;
      sourcePath: string;
      record: StoredCircuit;
    }): Promise<void> {
      const paths = resolveStorePaths(rootDir, identityKey, sourcePath);
      const keyHash = hashKey(identityKey);
      try {
        await mkdir(paths.dir, { recursive: true });
        // 写入顺序即崩溃语义：先修正记录，再几何（没有几何时删除旧文件，避免读到上一版的过期几何），
        // 最后写 ir.json 作为提交点。任何一步失败都不会留下「新 IR + 旧几何」的组合。
        // corrections.json 是数组、layout 是服务内部接口类型，不满足 atomicWriteJson 的 Record 入参，
        // 因此统一用 atomicWriteText（同样的临时文件 + rename + 文件锁）。
        await atomicWriteText(
          paths.correctionsPath,
          `${JSON.stringify(record.corrections, null, 2)}\n`,
        );
        if (record.layout) {
          await atomicWriteText(paths.layoutPath, `${JSON.stringify(record.layout, null, 2)}\n`);
        } else {
          await rm(paths.layoutPath, { force: true });
        }
        await atomicWriteJson(paths.irPath, record.ir);
      } catch (error) {
        log.error(undefined, "持久化电路 IR 失败", {
          key: keyHash,
          revision: record.ir.revision,
          error: describeError(error),
        });
        throw new Error(`io_error: 写入电路 IR 失败：${describeError(error)}`);
      }

      log.info(undefined, "持久化电路 IR 完成", {
        key: keyHash,
        revision: record.ir.revision,
        corrections: record.corrections.length,
        hasLayout: record.layout !== undefined,
      });
    },
  };
}
