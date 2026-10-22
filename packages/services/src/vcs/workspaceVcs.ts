import type {
  SetWorkspaceVcsOverrideRequest,
  WorkspaceVcsInfo,
  WorkspaceVcsRequest,
} from "@escode/shared";
import { ServiceChannels } from "@escode/shared";
import { createServiceDescriptor } from "../descriptors.js";

/**
 * workspace VCS 类型的唯一判定入口（spec：docs/svn/architecture.md）。
 * 只做目录探测与覆盖值读写，不缓存任何 Git / SVN 业务状态。
 */
export interface IWorkspaceVcsService {
  getWorkspaceVcsInfo(params: WorkspaceVcsRequest): Promise<WorkspaceVcsInfo>;
  setWorkspaceVcsOverride(params: SetWorkspaceVcsOverrideRequest): Promise<WorkspaceVcsInfo>;
}

export const IWorkspaceVcsService = createServiceDescriptor<IWorkspaceVcsService>(
  ServiceChannels.WorkspaceVcs,
);
