import { createHash } from "node:crypto";
import { readFile, stat, writeFile } from "node:fs/promises";
import { resolve, sep } from "node:path";
import type { CircuitSnapshotPort, CircuitSourcePort } from "../app/ports.js";

/**
 * workspace 源文件读取与导出快照写入适配器（docs/circuit/README.md §3.4、§2 规则 8）。
 * 只读 + 单点写入；源文件与导出路径都必须先证明落在 workspace 根目录内，避免越权访问宿主文件。
 */

/** 单文件读取上限：超过 16 MiB 的源文件（多为位图/PDF）不读进内存，直接报 too_large。 */
const MAX_SOURCE_BYTES = 16 * 1024 * 1024;

/** 已带错误码前缀的消息直接透传，避免二次包装改变错误码。 */
const CODE_PREFIX =
  /^(?:unavailable|not_found|unsupported_format|invalid_ir|stale_revision|too_large|io_error): /u;

function hasCodePrefix(error: unknown): error is Error {
  return error instanceof Error && CODE_PREFIX.test(error.message);
}

function errorCodeOf(error: unknown): string | undefined {
  if (typeof error !== "object" || error === null || !("code" in error)) {
    return undefined;
  }
  const code = (error as { code?: unknown }).code;
  return typeof code === "string" ? code : undefined;
}

/** ENOENT → not_found，其余 → io_error；已带前缀的错误原样返回。 */
function toSourceError(error: unknown, relativePath: string): Error {
  if (hasCodePrefix(error)) {
    return error;
  }
  if (errorCodeOf(error) === "ENOENT") {
    return new Error(`not_found: 源文件不存在：${relativePath}`);
  }
  return new Error(
    `io_error: 读取源文件失败：${relativePath}（${error instanceof Error ? error.message : String(error)}）`,
  );
}

/** 写入失败一律 io_error：导出目标的父目录不存在也属于写入失败，不隐式创建。 */
function toSnapshotError(error: unknown, relativePath: string): Error {
  if (hasCodePrefix(error)) {
    return error;
  }
  return new Error(
    `io_error: 写入导出快照失败：${relativePath}（${error instanceof Error ? error.message : String(error)}）`,
  );
}

/**
 * 校验路径是 workspace 相对路径：拒绝空串、绝对路径、盘符、UNC、`..` 段与空段。
 * 规则与 shared 的 circuitWorkspacePathSchema 对齐（路径去空格后判断，与 schema 的 trim 一致）。
 */
function assertWorkspaceRelative(relativePath: string): string {
  const value = relativePath.trim();
  if (!value) {
    throw new Error("io_error: 源文件路径为空");
  }
  if (value.includes("\0") || /[\r\n]/u.test(value)) {
    throw new Error(`io_error: 源文件路径含非法字符：${value}`);
  }
  if (/^(?:\/|[A-Za-z]:[\\/]|\\\\)/u.test(value)) {
    throw new Error(`io_error: 源文件路径必须是 workspace 相对路径：${value}`);
  }
  const segments = value.split(/[\\/]/u);
  if (segments.includes("..") || segments.includes("")) {
    throw new Error(`io_error: 源文件路径含非法段：${value}`);
  }
  return value;
}

/** Windows 路径比较不区分大小写；resolve 已统一分隔符与 . / .. 折叠。 */
function normalizeForCompare(pathValue: string): string {
  return process.platform === "win32" ? pathValue.toLowerCase() : pathValue;
}

/** 解析后的绝对路径必须落在 workspace 根内（前缀比较，根自身不算命中）。 */
function isInsideWorkspace(root: string, target: string): boolean {
  const comparableRoot = normalizeForCompare(root);
  const comparableTarget = normalizeForCompare(target);
  if (comparableTarget === comparableRoot) {
    return false;
  }
  const prefix = comparableRoot.endsWith(sep) ? comparableRoot : `${comparableRoot}${sep}`;
  return comparableTarget.startsWith(prefix);
}

/** 越界、绝对路径、盘符与 UNC 一律抛 `io_error`（调用方不区分「非法」与「读失败」）。 */
function resolveWorkspaceFile(workspacePath: string, relativePath: string): string {
  const value = assertWorkspaceRelative(relativePath);
  const root = resolve(workspacePath);
  const target = resolve(root, value);
  if (!isInsideWorkspace(root, target)) {
    throw new Error(`io_error: 路径不在 workspace 内：${value}`);
  }
  return target;
}

export function createWorkspaceSource(): CircuitSourcePort {
  /** 读取前检查：常规文件、大小上限（避免巨文件占满 Host 内存）。 */
  async function readSourceFile(workspacePath: string, relativePath: string): Promise<Buffer> {
    const target = resolveWorkspaceFile(workspacePath, relativePath);
    const label = relativePath.trim();
    try {
      // 先 stat 再读：把超大文件挡在 readFile 之前。
      const info = await stat(target);
      if (!info.isFile()) {
        throw new Error(`io_error: 源文件不是常规文件：${label}`);
      }
      if (info.size > MAX_SOURCE_BYTES) {
        throw new Error(`too_large: 源文件超过 16 MiB：${label}`);
      }
      return await readFile(target);
    } catch (error) {
      throw toSourceError(error, label);
    }
  }

  return {
    async readText({
      workspacePath,
      relativePath,
    }: {
      workspacePath: string;
      relativePath: string;
    }): Promise<string> {
      return (await readSourceFile(workspacePath, relativePath)).toString("utf-8");
    },

    async exists({
      workspacePath,
      relativePath,
    }: {
      workspacePath: string;
      relativePath: string;
    }): Promise<boolean> {
      let target: string;
      try {
        target = resolveWorkspaceFile(workspacePath, relativePath);
      } catch {
        // 路径越界按「不存在」处理：exists 只回答能否读取，不抛错。
        return false;
      }
      try {
        const info = await stat(target);
        return info.isFile();
      } catch {
        return false;
      }
    },

    hashContent(text: string): string {
      return createHash("sha256").update(text).digest("hex");
    },
  };
}

/**
 * 导出快照写入适配器（§2 规则 8）：只往 workspace 内写派生产物，同样先证明路径落在 workspace 根内。
 * 目录不存在时报 io_error，不隐式创建目录——导出目标由 Agent 明确给出（§5 circuit_export）。
 */
export function createWorkspaceSnapshotWriter(): CircuitSnapshotPort {
  return {
    async write({
      workspacePath,
      relativePath,
      content,
    }: {
      workspacePath: string;
      relativePath: string;
      content: string;
    }): Promise<void> {
      const target = resolveWorkspaceFile(workspacePath, relativePath);
      const label = relativePath.trim();
      try {
        await writeFile(target, content, "utf-8");
      } catch (error) {
        throw toSnapshotError(error, label);
      }
    },
  };
}
