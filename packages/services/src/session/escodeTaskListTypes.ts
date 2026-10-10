import type { WorkspacePurpose, ESCodeTaskMeta } from "@escode/shared";

export type ESCodeTaskListKind = "pinned" | "archived" | "timeline" | "active";
export type ESCodeTaskListSortBy = "created" | "updated";

export interface ESCodeTaskListWorkspaceScope {
  workspacePath: string;
  workspaceIdentity?: string;
  workspacePurpose?: WorkspacePurpose;
}

export interface ESCodeTaskListQuery {
  kind: ESCodeTaskListKind;
  workspaceScopes: ESCodeTaskListWorkspaceScope[];
  sortBy: ESCodeTaskListSortBy;
  search?: string;
  limit?: number;
}

export type ESCodeTaskListItem = ESCodeTaskMeta & {
  searchSnippet?: string;
  searchSnippets?: string[];
};

export interface ESCodeTaskListResult {
  items: ESCodeTaskListItem[];
  total: number;
  hasMore: boolean;
}

export type ESCodeTaskGroupColor =
  | "gray"
  | "red"
  | "orange"
  | "yellow"
  | "green"
  | "blue"
  | "purple";

export interface ESCodeTaskGroup {
  id: string;
  title: string;
  color: ESCodeTaskGroupColor;
  createdAt: number;
  updatedAt: number;
}

export interface ESCodeGroupedTaskRef {
  workspacePath: string;
  workspaceIdentity?: string;
  taskId: string;
}

export type ESCodeGroupedTaskViewTopLevelNodeRef =
  | { type: "group"; groupId: string }
  | { type: "task"; task: ESCodeGroupedTaskRef };

export type ESCodeGroupedTaskViewNode =
  | {
      type: "group";
      group: ESCodeTaskGroup;
      tasks: ESCodeTaskListItem[];
      sortOrder?: number;
    }
  | {
      type: "task";
      task: ESCodeTaskListItem;
      sortOrder?: number;
    };

export interface ESCodeGroupedTaskView {
  nodes: ESCodeGroupedTaskViewNode[];
}

export interface ESCodeGroupedTaskViewQuery {
  workspaceScopes: ESCodeTaskListWorkspaceScope[];
  includeAllWorkspaces?: boolean;
}

// ── grouped 原始结构（不 join tasks 表）──
// grouped 视图的任务数据源迁到 sessions-index 后，服务端只提供分组结构
// （task_groups / task_group_members / task_group_view_node_orders），
// 由客户端与 sessions-index 会话做 join。

/** 组成员引用（不含任务 meta；task 内容由 sessions-index 提供）。 */
export interface ESCodeGroupedTaskViewStructureMember {
  groupId: string;
  /** 服务端口径 workspaceKey（resolveWorkspaceKey：identity ?? path），join 匹配键。 */
  workspaceKey: string;
  workspacePath: string;
  workspaceIdentity?: string;
  taskId: string;
  /** null = 尚未落 sort_order（新加入组）；客户端按 addedAt 降序补内存序。 */
  sortOrder: number | null;
  addedAt: number;
}

/** 顶层节点排序（task_group_view_node_orders，node_key 已解析为结构化引用）。 */
export type ESCodeGroupedTaskViewStructureTopOrder =
  | { type: "group"; groupId: string; sortOrder: number }
  | { type: "task"; workspaceKey: string; taskId: string; sortOrder: number };

export interface ESCodeGroupedTaskViewStructure {
  /** 已按 workspaceScopes 可见性过滤的 group（bootstrap workspace group 只在其 workspace 可见）。 */
  groups: ESCodeTaskGroup[];
  /** 全量组成员（含不可见 group 的成员——顶层排除规则需要全量判断）。 */
  members: ESCodeGroupedTaskViewStructureMember[];
  topLevelOrders: ESCodeGroupedTaskViewStructureTopOrder[];
}

export interface ESCodeGroupedTaskViewOrderInput {
  workspaceScopes: ESCodeTaskListWorkspaceScope[];
  topLevelNodes: ESCodeGroupedTaskViewTopLevelNodeRef[];
  groups: Array<{
    groupId: string;
    taskRefs: ESCodeGroupedTaskRef[];
  }>;
}

export interface ESCodeWorkspaceEventSubscriptionParams {
  workspacePath: string;
  workspaceIdentity?: string;
}
