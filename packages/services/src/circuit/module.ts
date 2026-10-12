/**
 * circuit-service 模块清单（architecture-governance managed 模块）。
 * 纯 Host 侧服务：IR 的唯一所有者（ADR 0005）；domain 只放确定性解析与 IR 规则，
 * app 放状态与事件顺序，adapters 放应用数据目录、workspace 文件读取与源文件监听。
 */
export const circuitServiceModule = {
  id: "circuit-service",
  requires: ["services", "shared", "rpc", "circuit-protocol"],
  provides: ["circuit-service"],
  publicEntrypoints: ["index.ts", "contract.ts", "node.ts"],
} as const;
