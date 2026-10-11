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

/**
 * irId 由 (身份 key, 源路径) 派生（§3.2）：同一 workspace 下的同一源文件永远得到同一个 irId，
 * 服务重启后 Agent 手里的旧 irId 仍能被 findByIrId 找回。哈希函数由调用方注入（domain 不碰 IO）。
 */
export function circuitIrIdOf(location: CircuitLocation, hashContent: (text: string) => string): string {
  return `ir:${hashContent(`${location.identityKey}\n${location.sourcePath}`).slice(0, 16)}`;
}
