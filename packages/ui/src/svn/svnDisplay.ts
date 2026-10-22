import type {
  GitDiffResult,
  SvnBranchListResult,
  SvnBranchRef,
  SvnDiffResult,
  SvnFileChange,
  SvnIssueCode,
  SvnItemStatus,
} from "@escode/shared";
import type { GitPaneFileChange } from "@/hooks/useGitRepository.js";
import { joinFilePath } from "@/lib/path.js";
import {
  buildWorkspaceFileGitStatusByPath,
  buildWorkspaceFileIgnoredPathSet,
  type WorkspaceFileGitStatus,
} from "@/workspace-file-tree/model.js";

/** SVN 面板分组（spec：docs/svn/ui.md §3.2）。 */
export type SvnChangeGroupId = "conflicted" | "changes" | "unversioned";

export interface SvnChangeGroup {
  id: SvnChangeGroupId;
  changes: SvnFileChange[];
}

export function getSvnChangeGroupId(status: SvnItemStatus): SvnChangeGroupId | null {
  switch (status) {
    case "conflicted":
      return "conflicted";
    case "unversioned":
      return "unversioned";
    case "external":
    case "obstructed":
      // externals 属于其他工作副本，obstructed 需要命令行处理；第一期都只读展示在改动组。
      return "changes";
    default:
      return "changes";
  }
}

export function groupSvnChanges(changes: readonly SvnFileChange[]): SvnChangeGroup[] {
  const groups: Record<SvnChangeGroupId, SvnFileChange[]> = {
    conflicted: [],
    changes: [],
    unversioned: [],
  };
  for (const change of changes) {
    const groupId = getSvnChangeGroupId(change.status);
    if (groupId) {
      groups[groupId].push(change);
    }
  }
  const order: SvnChangeGroupId[] = ["conflicted", "changes", "unversioned"];
  return order
    .map((id) => ({
      id,
      changes: [...groups[id]].sort((left, right) => left.path.localeCompare(right.path)),
    }))
    .filter((group) => group.changes.length > 0);
}

/** 可勾选提交：冲突、externals、obstructed 不能直接提交。 */
export function isSvnChangeCommittable(change: SvnFileChange): boolean {
  return !["conflicted", "external", "obstructed"].includes(change.status);
}

/** 文件历史：已纳入版本控制且已在仓库中存在的路径（scheduled add / ignored 无历史）。 */
export function canShowSvnFileHistory(change: SvnFileChange): boolean {
  return !["unversioned", "added", "external", "obstructed"].includes(change.status);
}

/** Blame：历史可用的文件且当前修订确实存在（删除 / 丢失文件只有历史）。 */
export function canShowSvnBlame(change: SvnFileChange): boolean {
  return (
    canShowSvnFileHistory(change) &&
    !change.isDirectory &&
    !["deleted", "missing"].includes(change.status)
  );
}

/** 默认勾选：已纳入版本控制的可提交改动；未纳入版本控制的文件默认不勾选。 */
export function isSvnChangeCheckedByDefault(change: SvnFileChange): boolean {
  return isSvnChangeCommittable(change) && change.status !== "unversioned";
}

/**
 * 刷新后保留仍存在路径的勾选，新出现的路径按默认值；消失的路径自然丢弃。
 * 勾选是 UI 局部状态，不回写服务端。
 */
export function reconcileSvnSelection(
  previous: ReadonlyMap<string, boolean>,
  changes: readonly SvnFileChange[],
): Map<string, boolean> {
  const next = new Map<string, boolean>();
  for (const change of changes) {
    if (!isSvnChangeCommittable(change)) {
      continue;
    }
    next.set(change.path, previous.get(change.path) ?? isSvnChangeCheckedByDefault(change));
  }
  return next;
}

export function getSelectedSvnPaths(selection: ReadonlyMap<string, boolean>): string[] {
  return [...selection.entries()].filter(([, checked]) => checked).map(([path]) => path);
}

export type SvnCommitBlockReason = "conflicts" | "nothing-selected" | null;

export function getSvnCommitBlockReason(
  changes: readonly SvnFileChange[],
  selectedPaths: readonly string[],
): SvnCommitBlockReason {
  if (changes.some((change) => change.status === "conflicted")) {
    return "conflicts";
  }
  if (selectedPaths.length === 0) {
    return "nothing-selected";
  }
  return null;
}

const STATUS_LETTERS: Record<SvnItemStatus, string> = {
  modified: "M",
  added: "A",
  deleted: "D",
  replaced: "R",
  unversioned: "?",
  missing: "!",
  conflicted: "C",
  obstructed: "~",
  external: "X",
  "props-modified": "M",
};

export function getSvnStatusLetter(status: SvnItemStatus): string {
  return STATUS_LETTERS[status];
}

export function summarizeSvnChanges(changes: readonly SvnFileChange[]): {
  added: number;
  removed: number;
  fileCount: number;
  conflictCount: number;
} {
  let added = 0;
  let removed = 0;
  let conflictCount = 0;
  for (const change of changes) {
    added += change.additions;
    removed += change.deletions;
    if (change.status === "conflicted") {
      conflictCount += 1;
    }
  }
  return { added, removed, fileCount: changes.length, conflictCount };
}

/** 顶部分支名：trunk 直接显示，tag 加前缀，非标准布局显示相对路径。 */
export function getSvnBranchLabel(branch: SvnBranchRef | undefined): string | null {
  if (!branch) {
    return null;
  }
  if (branch.kind === "tag") {
    return `tag: ${branch.name}`;
  }
  return branch.name;
}

export interface SvnMergeSourceOption {
  relativePath: string;
  label: string;
}

/** 合并源：标准布局取 trunk 与 ^/branches，排除当前分支（spec：docs/svn/ui.md §11）。 */
export function getSvnMergeSourceOptions(
  branches: Pick<SvnBranchListResult, "layout" | "branches"> | null,
  current: SvnBranchRef | null | undefined,
): SvnMergeSourceOption[] {
  if (!branches || branches.layout !== "standard") {
    return [];
  }
  return branches.branches
    .filter((branch) => branch.relativePath !== current?.relativePath)
    .map((branch) => ({
      relativePath: branch.relativePath,
      label: getSvnBranchLabel(branch) ?? branch.relativePath,
    }));
}

export type SvnMergeRangeError = "missing" | "invalid";

/** 起止修订是闭区间；两端必填、正整数且 start ≤ end。 */
export function validateSvnMergeRange(start: string, end: string): SvnMergeRangeError | null {
  const rawStart = start.trim();
  const rawEnd = end.trim();
  if (!rawStart || !rawEnd) {
    return "missing";
  }
  const startRevision = Number(rawStart);
  const endRevision = Number(rawEnd);
  if (
    !Number.isInteger(startRevision) ||
    !Number.isInteger(endRevision) ||
    startRevision < 1 ||
    endRevision < 1 ||
    startRevision > endRevision
  ) {
    return "invalid";
  }
  return null;
}

export function parseSvnMergeRange(
  start: string,
  end: string,
): { startRevision: number; endRevision: number } | null {
  if (validateSvnMergeRange(start, end) !== null) {
    return null;
  }
  return { startRevision: Number(start.trim()), endRevision: Number(end.trim()) };
}

export function getSvnIssueMessageId(issue: SvnIssueCode): string {
  return `svn.issue.${issue}`;
}

function inferKind(status: SvnItemStatus): GitPaneFileChange["kind"] {
  if (status === "added" || status === "unversioned") {
    return "added";
  }
  if (status === "deleted" || status === "missing") {
    return "deleted";
  }
  return "modified";
}

/** 复用 Git 面板的变更卡片：把 SVN 改动映射成与 VCS 无关的卡片输入。 */
export function toSvnChangeCardModel(
  workspacePath: string,
  change: SvnFileChange,
): GitPaneFileChange {
  return {
    path: joinFilePath(workspacePath, change.path),
    repoRelativePath: change.path,
    workspaceRelativePath: change.path,
    kind: inferKind(change.status),
    section: change.status === "conflicted" ? "conflicted" : "unstaged",
    added: change.additions,
    removed: change.deletions,
    isStaged: false,
    isUntracked: change.status === "unversioned",
    isConflicted: change.status === "conflicted",
    diff: null,
  };
}

/**
 * 去掉 svn diff 末尾的属性差异段：共享 DiffViewer 只解析文本 hunk，
 * `Property changes on:` 段会被误当成内容行。属性差异另行文字提示。
 */
export function stripSvnPropertySections(patch: string): string {
  const lines = patch.split(/\r?\n/);
  const kept: string[] = [];
  let skipping = false;
  for (const line of lines) {
    if (line.startsWith("Property changes on: ")) {
      skipping = true;
      continue;
    }
    if (line.startsWith("Index: ")) {
      skipping = false;
    }
    if (!skipping) {
      kept.push(line);
    }
  }
  return kept.join("\n").trimEnd();
}

export function toSvnDiffCardModel(
  diff: SvnDiffResult | null,
  fallbackSummary?: string,
): GitDiffResult | null {
  if (!diff) {
    return null;
  }
  if (diff.isBinary) {
    return {
      path: diff.path,
      availability: "binary",
      patch: null,
      beforeContent: null,
      afterContent: null,
    };
  }
  const patch = diff.patch ? stripSvnPropertySections(diff.patch) : "";
  if (!patch.includes("@@")) {
    return {
      path: diff.path,
      availability: diff.truncated ? "truncated" : "unavailable",
      patch: null,
      beforeContent: null,
      afterContent: null,
      summary: fallbackSummary ?? null,
    };
  }
  return {
    path: diff.path,
    availability: "patch",
    patch,
    beforeContent: null,
    afterContent: null,
  };
}

/** 历史筛选只作用于已加载条目（spec：docs/svn/ui.md §5）。 */
export function matchesSvnLogFilter(
  entry: { author: string; message: string; revision: number },
  query: string,
): boolean {
  const normalized = query.trim().toLowerCase();
  if (!normalized) {
    return true;
  }
  return (
    entry.author.toLowerCase().includes(normalized) ||
    entry.message.toLowerCase().includes(normalized) ||
    `r${entry.revision}` === normalized
  );
}

export function getSvnLogMessageTitle(message: string): string {
  return (
    message
      .split(/\r?\n/)
      .find((line) => line.trim().length > 0)
      ?.trim() ?? ""
  );
}

/**
 * 文件树标记：直接复用面板同一次 getChanges 的结果，不额外发起 svn 命令
 * （spec：docs/svn/ui.md §7）。冲突在树中按修改展示，具体冲突在 SVN 面板处理。
 */
export function buildSvnFileTreeDecorations(
  workspacePath: string,
  changes: readonly SvnFileChange[],
  ignoredPaths: readonly string[],
): { statusByPath: Map<string, WorkspaceFileGitStatus>; ignoredPathSet: Set<string> } {
  return {
    statusByPath: buildWorkspaceFileGitStatusByPath(
      changes
        .filter((change) => change.path !== "." && change.status !== "external")
        .map((change) => ({
          path: joinFilePath(workspacePath, change.path),
          isUntracked: change.status === "unversioned",
          kind: inferKind(change.status),
          section: change.status === "unversioned" ? ("untracked" as const) : ("unstaged" as const),
        })),
    ),
    ignoredPathSet: buildWorkspaceFileIgnoredPathSet(
      ignoredPaths.map((path) => joinFilePath(workspacePath, path)),
    ),
  };
}
