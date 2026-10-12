# OpenAI Responses Input Item Protocol

## Goal

ESCode 发给 `/responses` 的请求体只使用 OpenAI Responses 规范中合法的 input item 形态，使严格按规范校验的 Responses 实现（自建 vLLM 等）与 OpenAI 官方服务同样可用。

## Problem

ESCode 曾把 assistant 历史回放成 easy message 携带 `output_text` 文本块：

```json
{ "role": "assistant", "content": [{ "type": "output_text", "text": "..." }] }
```

OpenAI 官方服务容忍这个混合形态（Node 侧 `@ai-sdk/openai` 就是这么发的），但它是规范外的写法，严格实现会直接拒绝。自建 vLLM 私有部署实测：第一轮成功，第二轮（携带 assistant 历史）起 HTTP 400：

```
body.input.6.EasyInputMessageParam.content.0.ResponseInputTextParam.type
  Input should be 'input_text', got 'output_text'
```

同一响应里 `body.input` 顶层也会出现 `Input should be a valid string`，那是 Pydantic 对 union 每个候选分支的失败枚举，根因只有上面这一条。

## Protocol Facts

- `EasyInputMessageParam.content` 是 `str | ResponseInputMessageContentListParam`；列表项只允许 `input_text` / `input_image` / `input_file` / `input_audio` 等 `input_*` 类型。`role` 可为 `user` / `assistant` / `system` / `developer`，content 类型与 role 无关。
- `output_text` 只合法于 `ResponseOutputMessageParam` 内，该类型 Required 字段为 `id`、`type: "message"`、`status`、`role: "assistant"`、`content`（`output_text | refusal`）；其中 `output_text` 带 `annotations`。
- vLLM 的 `input_item_parsing` 归一化只对 `type == "message"` 的 assistant item 生效（补 `id`、补 `status: "completed"`、补 `annotations: []`）；字符串 content 被明确放行为合法 `EasyInputMessageParam`。ESCode 发的混合形态两者都不满足，所以落到 easy message 分支后校验失败。

## Required Behavior

- 适用范围限定 OpenAI Responses transport（`providerKind === "openai"`、`apiFormat === "openai-responses"`），Node/AI SDK 链路与 Rust runtime 行为一致。
- assistant 历史按 `EasyInputMessage` 回放：
  - 纯文本单块折叠为 string content：`{ "role": "assistant", "content": "<text>" }`；
  - 其余情况（多块、含媒体）保留块数组，文本块一律 `input_text`。
- input item 中不出现 `output_text`。响应侧解析（把模型输出读成 `output_text`）与 `function_call` / `function_call_output` / `reasoning` item 回放保持不变。
- `store` / `include: ["reasoning.encrypted_content"]` / 加密推理 item 回放 / 工具结果 `call_id` 关联语义不变。
- Anthropic Messages 与 Chat Completions 投影不受影响；`system` / `user` item 现状不变（`user` 本来就使用 `input_text`）。

## Rejected Alternative

合成完整 `ResponseOutputMessage` item（`type: "message"` + 伪造 `id` + `status: "completed"` + `annotations: []`）也能通过严格校验，但需要为每条 assistant 文本构造不稳定的 item id，且只实现 easy message 分支的第三方 Responses 端点会失败。ESCode 目前只在 reasoning 上保留 provider item id（`transform.ts` 的 `providerOptionsForReasoningBlock`），assistant 文本没有可复用的 id，因此选择 easy message 形态。

## Verification

- Node 链路：`normalizeOpenAIResponsesInput` 单测（单块折叠、多块改 `input_text`、真实 output item 不动、`function_call_output` 不动），wire 测试断言携带 assistant 历史 + 工具结果的请求体不含 `output_text` 且 assistant 文本为 string content。
- Rust runtime：`model_media::responses` 的文本块恒为 `input_text`；`model_protocol` 单测覆盖 assistant 数组内容。
- 邻接不变：Anthropic Messages（system 块、thinking 回放、tool_result）与 Responses 流式解析用例保持通过。
- 命令：`pnpm typecheck`、`pnpm lint`、`pnpm test:escode-cli-rust`。
