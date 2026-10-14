// SSE 应答助手（自 escode-cli-rust-fixture.ts 拆出：仓库 lint 单文件 ≤400 行）；供差分用例模拟模型服务。
import type { ServerResponse } from "node:http";
import type { Message } from "./escode-cli-rust-fixture.js";

export function event(res: ServerResponse, delta: Message) {
  res.write(`data: ${JSON.stringify({ choices: [{ delta }] })}

`);
}
export function end(res: ServerResponse, reason: string) {
  res.end(
    `data: ${JSON.stringify({ choices: [{ delta: {}, finish_reason: reason }], usage: { prompt_tokens: 10, completion_tokens: 4 } })}

data: [DONE]

`,
  );
}
