import { createCircuitService, type CircuitService } from "./app/circuitService.js";
import { createFsCircuitStore } from "./adapters/fsCircuitStore.js";
import { createFsSourceWatcher } from "./adapters/sourceWatcher.js";
import { createWorkspaceSource } from "./adapters/workspaceFiles.js";

export { createCircuitService } from "./app/circuitService.js";
export type { CircuitService, CreateCircuitServiceOptions } from "./app/circuitService.js";

/**
 * Host 侧装配：应用数据目录持久化（§3.2）+ workspace 文件读取 + 源文件监听（§3.4）。
 * 只有 Desktop Local Host / 远程 workspace 的 Host 会创建它；纯 CLI 没有 broker 材料，工具返回 unavailable（§3.5）。
 */
export function createLocalCircuitService(): CircuitService {
  return createCircuitService({
    ports: {
      store: createFsCircuitStore(),
      source: createWorkspaceSource(),
      watcher: createFsSourceWatcher(),
    },
  });
}
