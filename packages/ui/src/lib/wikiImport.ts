// 知识库「导入文档」的纯逻辑（apps/escode-cli-rust/docs/llm-wiki/01-spec.md §11「导入入口」）。
import type { WikiScope } from "@escode/shared/escode-protocol-v4";

/** 文本框里的路径：每行一个，去空白、去重，保持顺序。 */
export function parseImportPaths(text: string): string[] {
  const seen = new Set<string>();
  const paths: string[] = [];
  for (const line of text.split(/\r?\n/)) {
    const path = line.trim();
    if (path && !seen.has(path)) {
      seen.add(path);
      paths.push(path);
    }
  }
  return paths;
}

/** 选择器追加的路径并入文本框（已存在的不重复追加）。 */
export function appendImportPaths(text: string, added: readonly string[]): string {
  const merged = [...parseImportPaths(text)];
  for (const path of added.map((p) => p.trim()).filter(Boolean)) {
    if (!merged.includes(path)) merged.push(path);
  }
  return merged.join("\n");
}

/**
 * 是否提供系统文件选择框：只有平台能返回本机绝对路径、且 runtime 也在本机（非远程 workspace）时，
 * 选出的路径才能被 runtime 读取；其余情况只能输入路径。
 */
export function canPickImportPaths(options: {
  canSelectFilePath: boolean | undefined;
  remoteSessionId?: string | null;
  remoteTarget?: unknown;
}): boolean {
  return Boolean(options.canSelectFilePath) && !options.remoteSessionId && !options.remoteTarget;
}

export type WikiImportStatus =
  | "imported"
  | "updated"
  | "unchanged"
  | "degraded"
  | "skipped"
  | "failed";

export interface WikiImportRow {
  path: string;
  status: WikiImportStatus | "unknown";
  error?: string;
  created: number;
  updated: number;
  proposals: number;
}

const STATUSES = new Set<string>([
  "imported",
  "updated",
  "unchanged",
  "degraded",
  "skipped",
  "failed",
]);

function count(value: unknown): number {
  return typeof value === "number" && Number.isFinite(value) ? value : 0;
}

/** runtime ingest 报告 → 展示行（items 是协议里的 unknown[]，逐字段校验）。 */
export function toImportRows(items: readonly unknown[]): WikiImportRow[] {
  return items.flatMap((item) => {
    if (!item || typeof item !== "object") return [];
    const record = item as Record<string, unknown>;
    if (typeof record.path !== "string") return [];
    const status =
      typeof record.status === "string" && STATUSES.has(record.status) ? record.status : "unknown";
    return [
      {
        path: record.path,
        status: status as WikiImportRow["status"],
        error: typeof record.error === "string" ? record.error : undefined,
        created: count(record.created),
        updated: count(record.updated),
        proposals: count(record.proposals),
      },
    ];
  });
}

export function importRowTone(
  status: WikiImportRow["status"],
): "success" | "warning" | "destructive" | "neutral" {
  switch (status) {
    case "imported":
    case "updated":
      return "success";
    case "degraded":
    case "skipped":
      return "warning";
    case "failed":
      return "destructive";
    default:
      return "neutral";
  }
}

export interface WikiImportSummary {
  files: number;
  created: number;
  updated: number;
  proposals: number;
  failed: number;
  skipped: number;
}

/** 结果页顶部的本地化摘要（runtime 的 summary 是英文诊断串，不直接展示给用户）。 */
export function summarizeImportRows(rows: readonly WikiImportRow[]): WikiImportSummary {
  return rows.reduce<WikiImportSummary>(
    (sum, row) => ({
      files: sum.files + 1,
      created: sum.created + row.created,
      updated: sum.updated + row.updated,
      proposals: sum.proposals + row.proposals,
      failed: sum.failed + (row.status === "failed" ? 1 : 0),
      skipped: sum.skipped + (row.status === "skipped" ? 1 : 0),
    }),
    { files: 0, created: 0, updated: 0, proposals: 0, failed: 0, skipped: 0 },
  );
}

/** 目标作用域的默认值：项目 Wiki（随仓库、只对本项目可见），全局需用户主动选择。 */
export const DEFAULT_IMPORT_SCOPE: WikiScope = "project";
