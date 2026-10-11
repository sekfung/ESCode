// ============================================================
// Token Estimation
// ============================================================

// 实现已上移到 @escode/shared：Renderer 的生成速度读数与 CLI 上下文预算必须共用同一份估算口径
// （流式协议不携带逐 chunk token 数，两侧都只能按流出文本估算）。保留本文件与导出名，
// 避免改动包内既有的 `../utils.js` import 路径。
export { estimateTokens } from "@escode/shared";
