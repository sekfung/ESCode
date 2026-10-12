# Anthropic Messages Thinking Budget Protocol

## Goal

ESCode 发给 Anthropic Messages 兼容端点的 `thinking.enabled` 请求必须带规范要求的 `budget_tokens`，使严格按 Anthropic schema 校验的实现（自建 vLLM 等）与容忍端（z.ai / GLM 风格网关）都能接受同一份正文。

## Problem

ESCode 的模型选项映射（`config/provider/escode-builtin.json` 的 `reasoningLevel` 表达式）只输出档位语义，例如：

```json
{ "thinking": { "type": "enabled" }, "output_config": { "effort": "high" } }
```

Anthropic 规范里 `ThinkingConfigEnabled` 的必填字段是 `type` 与 `budget_tokens`（1024 ≤ budget_tokens < max_tokens）。自建 vLLM（Anthropic 兼容端点）实测直接 400：

```
1 validation error: {'type': 'missing', 'loc': 'body.thinking.enabled.budget_tokens', 'msg': 'Field required'}
```

映射表达式无法自己算出预算：表达式只允许引用自身选项变量（`reasoningLevel == "…" ? … : …`），引用 `maxOutputTokens` 直接报 `unknown identifier`；写死固定预算又和小输出上限的请求（标题、压缩）冲突——这正是仓库此前把 OpenRouter 模板改成 `adaptive` 的原因（`packages/provider-node/test/openrouter-reasoning-contract.test.ts` 的「不引入与小输出上限冲突的固定预算」）。

## Required Behavior

- 适用范围：`apiFormat === "anthropic-messages"`（Node/AI SDK 链路与 Rust runtime 一致）。
- 只处理 `thinking.type === "enabled"`：`adaptive` / `disabled` / 无 `thinking` 一律不动。
- 已给出正整数 `budget_tokens` 时尊重配置，不覆盖；非正整数视为缺失。
- 缺失时按 `max_tokens` 推导：`budget = min(max(1024, floor(max_tokens / 2)), max_tokens - 1)`。
- `max_tokens` 缺失或非正整数时不动正文；`max_tokens <= 1024` 时移除 `thinking` 字段（规范下限 1024 无法满足，省略即不启用思考，避免发出必然被拒的正文）。
- 只在最终正文上补齐一次：模型选项映射（Option Map 是 reasoning 请求字段的唯一权威）打补丁之后、请求正文捕获之前运行，`rawRequestBodyCapture` 记录的就是真正发出的正文。
- 映射数据与响应解析不变；`output_config` 等其他字段保持原样。

## Verification

- Node 链路：`ensureAnthropicThinkingBudget` 单测（补齐、保留显式预算、`adaptive`/`disabled` 不动、`max_tokens` 过小移除 thinking、缺 `max_tokens` 不动）；wire 测试用内置规则解析 `deepseek-v4.1-flash` + `anthropic-messages` + 非 disabled 档位，断言实际请求体 `thinking = {type: "enabled", budget_tokens: N}` 且 `1024 ≤ N < max_tokens`。
- Rust runtime：`anthropic_body::ensure_thinking_budget` 单测覆盖同一组规则。
- 邻接不变：OpenRouter 的 `adaptive` 映射契约、`thinking: {"type":"disabled"}` 关闭路径、其他协议的字段不变。
- 命令：`pnpm typecheck`、`pnpm lint`、`pnpm test:escode-cli-rust`。
