import type { CircuitWorkspaceRef } from "../contract.js";

/**
 * IR 的记录位置：身份 key + 源文件路径（§3.2、Workspace Identity 规则）。
 * 身份 key 统一为 `workspaceIdentity?.trim() || workspacePath`，服务内部所有 map/队列都以它 + 源路径为键。
 */
export interface CircuitLocation extends CircuitWorkspaceRef {
  identityKey: string;
  sourcePath: string;
}

export function circuitLocationOf(
  params: CircuitWorkspaceRef & { path: string },
): CircuitLocation {
  return {
    workspacePath: params.workspacePath,
    ...(params.workspaceIdentity ? { workspaceIdentity: params.workspaceIdentity } : {}),
    ...(params.remoteSessionId ? { remoteSessionId: params.remoteSessionId } : {}),
    identityKey: params.workspaceIdentity?.trim() || params.workspacePath,
    sourcePath: params.path,
  };
}

export function circuitLocationKey(location: CircuitLocation): string {
  return `${location.identityKey}\u0000${location.sourcePath}`;
}
