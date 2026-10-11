/**
 * circuit-protocol 模块清单（architecture-governance managed 模块）。
 * 纯 domain：Circuit IR v1、识别草稿与 MCP 工具线协议的常量与 zod schema，零 IO。
 * IR 的唯一所有者是 Host 内的 CircuitService（ADR 0005），本模块不持有状态。
 */
export const circuitProtocolModule = {
  id: "circuit-protocol",
  requires: [],
  provides: ["circuit-protocol"],
  publicEntrypoints: ["index.ts", "contract.ts"],
} as const;
