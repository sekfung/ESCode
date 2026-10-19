// 执行材料投影已迁到 v4 原生层，旧协议路径反向复用，避免两份实现漂移。
export { createModelExecutionContext } from "../escode-protocol-v4/model-execution-context.js";
