// ============================================================
// Context Source Port - host/workspace context boundary
// ============================================================

import type { ExecutionContext, TraceContext } from "../tracing/tracer.js";

export interface EnvInfo {
  cwd: string;
  platform: string;
  shell: string;
  osVersion: string;
  nodeVersion: string;
  // 执行模型不是环境事实；只能由当前 model step 的 Model 在 Prompt 渲染时提供。
  isGitRepository?: boolean;
  gitBranch?: string;
  gitMainBranch?: string;
  gitUser?: string;
  gitStatus?: "clean" | "dirty" | "not_repo";
  gitStatusLines?: string[];
  recentCommits?: string[];
  /**
   * SVN 工作副本快照（docs/svn/architecture.md「Agent 集成」）。判定规则与 UI 的
   * workspace-vcs 一致：最近的 `.svn` 比 `.git` 更近时才收集，此时不收集 Git 快照。
   */
  svn?: SvnEnvSnapshot;
}

export interface SvnEnvSnapshot {
  url?: string;
  revision?: number;
  /** 由 URL 推导的分支（trunk / 分支名 / tag: 名称 / 相对路径）。 */
  branch?: string;
  /** `svn status` 输出，按 2000 字符截断。 */
  statusLines: string[];
  /** 最近 5 条日志摘要；访问服务器失败或超时时为空。 */
  recentLogs: string[];
}

export interface UserInstructionsOptions {
  workingDirectory: string;
  projectRoot?: string;
  priorityFiles?: string[];
  maxBytes?: number;
}

export type UserInstructionSourceScope = "user" | "workspace";

export interface ResolvedUserInstructionSource {
  scope: UserInstructionSourceScope;
  filePath: string;
  fileName: string;
  content: string;
  bytesRead: number;
  sizeBytes: number;
  truncated: boolean;
}

export interface ResolvedUserInstructions {
  filePath: string;
  fileName: string;
  content: string;
  bytesRead: number;
  sizeBytes: number;
  truncated: boolean;
  sources?: ResolvedUserInstructionSource[];
}

export type ProjectType = "node" | "python" | "rust" | "go" | "java" | "unknown";
export type PackageManager = "npm" | "yarn" | "pnpm" | "bun";

export interface ProjectContext {
  type: ProjectType;
  packageManager?: PackageManager;
  scripts?: Record<string, string>;
  buildFiles?: string[];
}

export interface ContextSourceRequest {
  workingDirectory: string;
  currentDate?: string;
  effectiveShellDisplayName?: string;
  envInfo?: EnvInfo;
  userInstructions?: UserInstructionsOptions;
  projectContext?: ProjectContext;
  trace?: TraceContext;
}

export interface ContextSourceDiagnostic {
  code: string;
  message: string;
  path?: string;
}

export interface ContextSourceSnapshot {
  workingDirectory: string;
  envInfo: EnvInfo;
  currentDate?: string;
  userInstructions?: ResolvedUserInstructions;
  projectContext?: ProjectContext;
  diagnostics: ContextSourceDiagnostic[];
}

export interface ContextSourceResolveOptions {
  signal?: AbortSignal;
  context?: ExecutionContext;
}

export interface ContextSourcePort {
  resolveContextSources(
    request: ContextSourceRequest,
    options?: ContextSourceResolveOptions,
  ): Promise<ContextSourceSnapshot>;
}
