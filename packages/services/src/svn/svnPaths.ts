import { open, readFile, stat } from "node:fs/promises";
import { isAbsolute, relative, resolve } from "node:path";
import { DEFAULT_SVN_DIFF_BYTES, normalizeSvnPath } from "./config.js";

/** 文本冲突辅助文件的固定后缀，作为 info 查询失败时的兜底识别。 */
export const CONFLICT_ARTIFACT_SUFFIX = /\.(mine|working|r\d+|merge-left\.r\d+|merge-right\.r\d+)$/;
const UNVERSIONED_STAT_MAX_BYTES = 1024 * 1024;

/**
 * svn 把路径中最后一个 `@` 之后的内容当作 peg revision，文件名本身带 `@`
 * （如 `icon@2x.png`）时必须追加一个 `@` 才能按字面解析。只在需要时追加：
 * `.@` 会被 svn 规范化成名为 `@` 的子节点。
 */
export function localTarget(path: string): string {
  return path.includes("@") ? `${path}@` : path;
}

export function toWorkspaceRelative(workspacePath: string, absoluteOrRelative: string): string {
  const absolute = isAbsolute(absoluteOrRelative)
    ? absoluteOrRelative
    : resolve(workspacePath, absoluteOrRelative);
  return normalizeSvnPath(relative(workspacePath, absolute)) || ".";
}

export function isSafeRelativePath(path: string): boolean {
  const normalized = normalizeSvnPath(path);
  return (
    normalized.length > 0 &&
    !isAbsolute(normalized) &&
    !/^[a-zA-Z]:/.test(normalized) &&
    !normalized.split("/").includes("..")
  );
}

export function splitDiffByIndex(patch: string): Map<string, string> {
  const sections = new Map<string, string>();
  let currentPath: string | null = null;
  let buffer: string[] = [];
  const flush = () => {
    if (currentPath !== null) {
      sections.set(currentPath, buffer.join("\n"));
    }
  };
  for (const line of patch.split(/\r?\n/)) {
    if (line.startsWith("Index: ")) {
      flush();
      currentPath = normalizeSvnPath(line.slice("Index: ".length).trim());
      buffer = [line];
      continue;
    }
    buffer.push(line);
  }
  flush();
  return sections;
}

export async function mapWithConcurrency<T, R>(
  items: readonly T[],
  concurrency: number,
  fn: (item: T) => Promise<R>,
): Promise<R[]> {
  const results: R[] = Array.from({ length: items.length });
  let next = 0;
  const workers = Array.from({ length: Math.min(concurrency, items.length) }, async () => {
    while (next < items.length) {
      const index = next;
      next += 1;
      results[index] = await fn(items[index]!);
    }
  });
  await Promise.all(workers);
  return results;
}

export async function isDirectory(path: string): Promise<boolean> {
  return await stat(path)
    .then((info) => info.isDirectory())
    .catch(() => false);
}

export async function readUnversionedFileStat(
  absolutePath: string,
): Promise<{ additions: number; isBinary: boolean; isDirectory: boolean }> {
  try {
    const info = await stat(absolutePath);
    if (info.isDirectory()) {
      return { additions: 0, isBinary: false, isDirectory: true };
    }
    if (info.size > UNVERSIONED_STAT_MAX_BYTES) {
      return { additions: 0, isBinary: false, isDirectory: false };
    }
    const content = await readFile(absolutePath);
    if (content.includes(0)) {
      return { additions: 0, isBinary: true, isDirectory: false };
    }
    const text = content.toString("utf-8");
    const lines =
      text.length === 0 ? 0 : text.split(/\r?\n/).length - (text.endsWith("\n") ? 1 : 0);
    return { additions: lines, isBinary: false, isDirectory: false };
  } catch {
    return { additions: 0, isBinary: false, isDirectory: false };
  }
}

/** 为未纳入版本控制的文件合成“全部新增”的 patch，让界面沿用同一套 diff 展示。 */
export async function buildUnversionedPatch(
  absolutePath: string,
  displayPath: string,
): Promise<{ patch: string | null; isBinary: boolean; truncated: boolean }> {
  const handle = await open(absolutePath, "r");
  try {
    const buffer = Buffer.alloc(DEFAULT_SVN_DIFF_BYTES);
    const { bytesRead } = await handle.read(buffer, 0, buffer.length, 0);
    const content = buffer.subarray(0, bytesRead);
    if (content.includes(0)) {
      return { patch: null, isBinary: true, truncated: false };
    }
    const info = await handle.stat();
    const lines = content.toString("utf-8").split(/\r?\n/);
    if (lines[lines.length - 1] === "") {
      lines.pop();
    }
    const patch = [
      `Index: ${displayPath}`,
      "===================================================================",
      `--- ${displayPath}\t(nonexistent)`,
      `+++ ${displayPath}\t(working copy)`,
      `@@ -0,0 +1,${lines.length} @@`,
      ...lines.map((line) => `+${line}`),
    ].join("\n");
    return { patch, isBinary: false, truncated: info.size > bytesRead };
  } finally {
    await handle.close();
  }
}
