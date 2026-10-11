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
 * irId → 记录位置的索引（§3.2）：同一位置换了源文件内容就是一份新 IR，旧 irId 必须从索引里删掉，
 * 否则拿着旧 irId 还能读到新 IR 的内容。确定性解析的 irId 不随内容变化，只有识别链路（内容哈希派生）会换。
 */
export function indexCircuitIrId(
  index: Map<string, string>,
  location: CircuitLocation,
  irId: string,
  previousIrId: string | undefined,
): void {
  if (previousIrId !== undefined && previousIrId !== irId) index.delete(previousIrId);
  index.set(irId, circuitLocationKey(location));
}

/**
 * irId 由 (身份 key, 源路径) 派生（§3.2）：同一 workspace 下的同一源文件永远得到同一个 irId，
 * 服务重启后 Agent 手里的旧 irId 仍能被 findByIrId 找回。哈希函数由调用方注入（domain 不碰 IO）。
 */
export function circuitIrIdOf(location: CircuitLocation, hashContent: (text: string) => string): string {
  return `ir:${hashContent(`${location.identityKey}\n${location.sourcePath}`).slice(0, 16)}`;
}

/**
 * 图片 / PDF 的 irId 还要带内容哈希（§3.2）：内容一变就是一份新的 IR，旧修正不迁移。
 * 同一份内容重新提交仍是同一个 irId，因此能沿用 `parse` 的 baseRevision 前置条件（§3.4）。
 */
export function circuitContentIrIdOf(
  location: CircuitLocation,
  contentHash: string,
  hash: (text: string) => string,
): string {
  return `ir:${hash(`${location.identityKey}\n${location.sourcePath}\n${contentHash}`).slice(0, 16)}`;
}
