import { event, end } from "./zcode-cli-rust-fixture.js";
import type { ServerResponse } from "node:http";

type Message = Record<string, any>;

// 会话标题 sidecar 的请求判定与应答（docs/specs/rust-session-title.md）。
/** 标题 sidecar 请求（system 首段为固定提示词）。 */
export function titleRequest(request: Message): boolean {
  // Anthropic 把 system 放在顶层 `system`，Responses 可能用 `instructions`；四处都要看。
  const { messages, input, system, instructions } = request ?? {};
  return JSON.stringify([messages, input, system, instructions]).includes(
    "Generate a concise title for this coding session",
  );
}
/**
 * 按客户端形态回答标题请求：Node 的 sidecar 走非流式 `generateText`，Rust 始终流式。
 * 用例的 `requests` 记录必须排除它，否则请求下标/条数会随运行时不同（此前只按非流式分支返回）。
 */
export function titleReply(res: ServerResponse, request: Message, content = "Title") {
  if (request.stream === false || request.stream === undefined) {
    res.writeHead(200, { "content-type": "application/json" });
    res.end(
      JSON.stringify({
        id: "title",
        object: "chat.completion",
        choices: [{ index: 0, message: { role: "assistant", content }, finish_reason: "stop" }],
        usage: { prompt_tokens: 1, completion_tokens: 1, total_tokens: 2 },
      }),
    );
    return;
  }
  res.writeHead(200, { "content-type": "text/event-stream" });
  event(res, { content });
  end(res, "stop");
}
