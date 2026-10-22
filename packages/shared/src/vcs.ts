/**
 * 与具体版本控制系统无关的 workspace VCS 判定结果。
 * 唯一判定入口是 IWorkspaceVcsService（spec：docs/svn/architecture.md）。
 */
export type WorkspaceVcsKind = "none" | "git" | "svn";

export type WorkspaceVcsOverride = "auto" | WorkspaceVcsKind;

export const WORKSPACE_VCS_OVERRIDES = [
  "auto",
  "none",
  "git",
  "svn",
] as const satisfies readonly WorkspaceVcsOverride[];

export interface WorkspaceVcsRequest {
  workspacePath: string;
  workspaceIdentity?: string;
}

export interface WorkspaceVcsInfo {
  kind: WorkspaceVcsKind;
  source: "detected" | "override";
  /** 当前覆盖值；未覆盖时为 auto。 */
  override: WorkspaceVcsOverride;
  /** 对应 VCS 的根目录；kind 为 none 或覆盖后目录不是对应仓库时缺省。 */
  root?: string;
}

export interface SetWorkspaceVcsOverrideRequest extends WorkspaceVcsRequest {
  override: WorkspaceVcsOverride;
}

/** 身份 key 统一规则：workspaceIdentity 优先，缺省回退到 workspacePath。 */
export function getWorkspaceVcsKey(params: WorkspaceVcsRequest): string {
  return params.workspaceIdentity?.trim() || params.workspacePath;
}

export interface VcsMarkerDepths {
  /** 最近 `.git` 所在目录距离文件系统根的深度；未找到为 null。 */
  gitDepth: number | null;
  svnDepth: number | null;
}

/**
 * 根据最近的 `.git` 与 `.svn` 所在深度裁决 VCS 类型：更深（离 workspace 更近）者优先，
 * 同层时 Git 优先。Host 服务与 Agent 快照共用这条规则，避免 UI 与 Agent 判断不一致。
 */
export function resolveDetectedVcsKind(depths: VcsMarkerDepths): WorkspaceVcsKind {
  const { gitDepth, svnDepth } = depths;
  if (gitDepth === null && svnDepth === null) {
    return "none";
  }
  if (svnDepth === null) {
    return "git";
  }
  if (gitDepth === null) {
    return "svn";
  }
  return svnDepth > gitDepth ? "svn" : "git";
}
