import type { Locale } from "./protocol.js";
import type { GitCommitMessageConversationContext } from "./git.js";

/** SVN 支持的类型定义；spec：docs/svn/architecture.md。 */

export const SVN_MIN_SUPPORTED_VERSION = "1.8.0";

export interface SvnRequest {
  workspacePath: string;
  workspaceIdentity?: string;
}

export interface SvnCredentials {
  username: string;
  password: string;
  /** false 时附加 --no-auth-cache，svn 不缓存这次输入的凭据。 */
  storeInSvnCache: boolean;
}

export type SvnAvailability =
  | { status: "available"; version: string; binaryPath: string }
  | { status: "not-found" }
  | { status: "unsupported-version"; version: string; binaryPath: string }
  | { status: "unsupported-host" };

export type SvnIssueCode =
  | "svn-unavailable"
  | "not-working-copy"
  | "working-copy-locked"
  | "out-of-date"
  | "auth-required"
  | "unresolved-conflicts"
  | "nothing-to-commit"
  | "invalid-request"
  | "unsupported-host"
  | "cancelled"
  | "timeout"
  | "unknown";

export interface SvnFailure {
  ok: false;
  issue: SvnIssueCode;
  message: string;
}

export type SvnResult<T extends object = object> = ({ ok: true } & T) | SvnFailure;

export type SvnBranchKind = "trunk" | "branch" | "tag" | "other";

export interface SvnBranchRef {
  kind: SvnBranchKind;
  /** trunk / 分支名 / tag 名；other 时为相对仓库根的路径。 */
  name: string;
  /** 相对仓库根的路径，不以 / 开头，例如 `branches/feature`。 */
  relativePath: string;
}

export interface SvnWorkingCopyInfo {
  availability: SvnAvailability;
  isWorkingCopy: boolean;
  wcRoot?: string;
  url?: string;
  repositoryRoot?: string;
  repositoryUuid?: string;
  revision?: number;
  branch?: SvnBranchRef;
  /** 工作副本根是否正好是某个分支根；只有这时才允许 svn switch 切换分支。 */
  canSwitch?: boolean;
}

export type SvnItemStatus =
  | "modified"
  | "added"
  | "deleted"
  | "replaced"
  | "unversioned"
  | "missing"
  | "conflicted"
  | "obstructed"
  | "external"
  | "props-modified";

export type SvnConflictKind = "text" | "property" | "tree";

export interface SvnConflictInfo {
  kinds: SvnConflictKind[];
  /** 树冲突原因，只读展示。 */
  description?: string;
}

export interface SvnFileChange {
  /** 相对 workspacePath，分隔符统一为 /。 */
  path: string;
  status: SvnItemStatus;
  propsModified: boolean;
  isDirectory: boolean;
  conflict?: SvnConflictInfo;
  additions: number;
  deletions: number;
  isBinary: boolean;
}

export interface SvnChangesResult {
  changes: SvnFileChange[];
  /** svn:ignore 命中的路径，相对 workspacePath，仅用于文件树置灰。 */
  ignoredPaths: string[];
}

export interface SvnDiffRequest extends SvnRequest {
  /** 相对 workspacePath 的文件路径；查看修订 diff 时为仓库内路径。 */
  path: string;
  /** 提供时读取该修订引入的改动（svn diff -c）。 */
  revision?: number;
}

export interface SvnDiffResult {
  path: string;
  patch: string | null;
  isBinary: boolean;
  truncated: boolean;
}

export interface SvnLogRequest extends SvnRequest {
  limit?: number;
  /** 从该修订向前（更早）读取；缺省从 HEAD 开始。 */
  startRevision?: number;
  credentials?: SvnCredentials;
}

export type SvnChangedPathAction = "A" | "M" | "D" | "R";

export interface SvnLogChangedPath {
  action: SvnChangedPathAction;
  /** 仓库内路径，以 / 开头。 */
  path: string;
  kind?: "file" | "dir";
  copyFromPath?: string;
  copyFromRevision?: number;
}

export interface SvnLogEntry {
  revision: number;
  author: string;
  /** ISO 8601 */
  date: string;
  message: string;
  changedPaths: SvnLogChangedPath[];
}

export interface SvnLogResult {
  entries: SvnLogEntry[];
  hasMore: boolean;
}

export interface SvnRemoteStatusRequest extends SvnRequest {
  credentials?: SvnCredentials;
}

/**
 * 服务器侧状态。用 `svn status -u` 的过期条目判断是否需要更新，
 * 而不是比较修订号：自己刚提交的修订不会被误报为“服务器有新修订”。
 */
export interface SvnRemoteStatusResult {
  headRevision: number;
  outOfDateCount: number;
}

export interface SvnBranchListRequest extends SvnRequest {
  credentials?: SvnCredentials;
}

export interface SvnBranchListResult {
  current?: SvnBranchRef;
  /** 仓库根下存在 trunk 与 branches 时为 standard。 */
  layout: "standard" | "non-standard";
  branches: SvnBranchRef[];
}

export interface SvnCommitRequest extends SvnRequest {
  /** 相对 workspacePath；未纳入版本控制的路径会先执行 svn add。 */
  paths: string[];
  message: string;
  credentials?: SvnCredentials;
}

export interface SvnCommitResult {
  revision: number;
  addedPaths: string[];
}

export interface SvnUpdateRequest extends SvnRequest {
  credentials?: SvnCredentials;
}

export interface SvnUpdateResult {
  revision: number;
  changedFileCount: number;
  conflictCount: number;
}

export interface SvnRevertRequest extends SvnRequest {
  paths: string[];
}

export type SvnResolveAccept = "mine-full" | "theirs-full" | "working";

export interface SvnResolveRequest extends SvnRequest {
  path: string;
  accept: SvnResolveAccept;
}

export interface SvnSwitchRequest extends SvnRequest {
  /** 相对仓库根，例如 `trunk`、`branches/feature`。 */
  relativePath: string;
  credentials?: SvnCredentials;
}

export interface SvnSwitchResult {
  revision: number;
  branch?: SvnBranchRef;
  conflictCount: number;
}

export interface SvnSetBinaryPathRequest {
  /** 空字符串表示清除手动路径、恢复自动查找。 */
  binaryPath: string;
}

export interface SvnGenerateCommitMessageRequest extends SvnRequest {
  paths: string[];
  locale?: Locale;
  conversationContext?: GitCommitMessageConversationContext;
}

export interface SvnGenerateCommitMessageResult {
  message: string;
  providerId: string;
  model: string;
}

const STANDARD_BRANCH_PREFIXES = [
  { prefix: "branches/", kind: "branch" },
  { prefix: "tags/", kind: "tag" },
] as const;

/**
 * 由工作副本 URL 相对仓库根的路径推导分支。只认标准布局（trunk / branches/* / tags/*），
 * 并只取该布局下的第一层目录名；其余一律视为 other，避免猜测非标准布局。
 */
export function deriveSvnBranchRef(relativePathInput: string): SvnBranchRef {
  const relativePath = relativePathInput.replace(/^\^?\/+/, "").replace(/\/+$/, "");
  if (relativePath === "trunk" || relativePath.startsWith("trunk/")) {
    return { kind: "trunk", name: "trunk", relativePath: "trunk" };
  }
  for (const { prefix, kind } of STANDARD_BRANCH_PREFIXES) {
    if (relativePath.startsWith(prefix)) {
      const name = relativePath.slice(prefix.length).split("/")[0] ?? "";
      if (name.length > 0) {
        return { kind, name, relativePath: `${prefix}${name}` };
      }
    }
  }
  return { kind: "other", name: relativePath || "/", relativePath };
}

/** 计算 url 相对 repositoryRoot 的路径，并做 URL 解码；不在仓库下时返回 null。 */
export function getSvnRelativeRepositoryPath(url: string, repositoryRoot: string): string | null {
  const root = repositoryRoot.replace(/\/+$/, "");
  if (url !== root && !url.startsWith(`${root}/`)) {
    return null;
  }
  const raw = url.slice(root.length).replace(/^\/+/, "");
  try {
    return decodeURIComponent(raw);
  } catch {
    return raw;
  }
}
