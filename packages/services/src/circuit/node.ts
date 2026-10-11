import { createCircuitService, type CircuitService } from "./app/circuitService.js";
import { createFsCircuitStore } from "./adapters/fsCircuitStore.js";
import { createPdfVectorExtractor } from "./adapters/pdfVector.js";
import { createFsSourceWatcher } from "./adapters/sourceWatcher.js";
import { createWorkspaceSnapshotWriter, createWorkspaceSource } from "./adapters/workspaceFiles.js";

export { createCircuitService } from "./app/circuitService.js";
export type { CircuitService, CreateCircuitServiceOptions } from "./app/circuitService.js";
// Agent 链路（§5）：Host 侧桥接与反向请求路由。装配层从这里取，避免跨模块深入 app/ 目录。
export { createCircuitAgentBridge, type CircuitAgentBridge } from "./app/circuitAgentBridge.js";
export { routeCircuitAgentRequest } from "./app/circuitAgentRequestRouter.js";

/**
 * Host 侧装配：应用数据目录持久化（§3.2）+ workspace 文件读取与导出快照写入 + 源文件监听（§3.4）
 * + 矢量 PDF 的文本/线段提取（§5 circuit_submit_recognition）。
 * 只有 Desktop Local Host / 远程 workspace 的 Host 会创建它；纯 CLI 没有 broker 材料，工具返回 unavailable（§3.5）。
 */
export function createLocalCircuitService(): CircuitService {
  return createCircuitService({
    ports: {
      store: createFsCircuitStore(),
      source: createWorkspaceSource(),
      watcher: createFsSourceWatcher(),
      snapshot: createWorkspaceSnapshotWriter(),
      vector: createPdfVectorExtractor(),
    },
  });
}
