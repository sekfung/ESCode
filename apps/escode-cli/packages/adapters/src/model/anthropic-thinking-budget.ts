import type { JsonObject, JsonValue } from "@escode/model-option-map";

/**
 * Anthropic Messages 规范要求 `thinking.type == "enabled"` 必带 `budget_tokens`（1024 ≤ budget < max_tokens）。
 * 模型选项映射只输出档位语义（映射表达式无法引用 maxOutputTokens），所以在最终正文上补齐；
 * 规则与证据：docs/anthropic-thinking-budget-protocol.md。
 */
export function ensureAnthropicThinkingBudget(body: JsonObject): JsonObject {
  const thinking = asObject(body.thinking);
  if (thinking?.type !== "enabled") return body;
  if (positiveInteger(thinking.budget_tokens) !== undefined) return body;

  const maxTokens = positiveInteger(body.max_tokens);
  if (maxTokens === undefined) return body;

  const budget = Math.min(Math.max(1024, Math.floor(maxTokens / 2)), maxTokens - 1);
  // max_tokens 撑不起规范下限：省略 thinking（不启用思考），不发必然被拒的正文。
  if (budget < 1024) return withoutThinking(body);

  return { ...body, thinking: { ...thinking, budget_tokens: budget } };
}

function withoutThinking(body: JsonObject): JsonObject {
  const rest: Record<string, JsonValue> = {};
  for (const [key, value] of Object.entries(body)) {
    if (key !== "thinking") rest[key] = value;
  }
  return rest;
}

function positiveInteger(value: JsonValue | undefined): number | undefined {
  if (typeof value !== "number" || !Number.isInteger(value) || value <= 0) return undefined;
  return value;
}

function asObject(value: JsonValue | undefined): JsonObject | undefined {
  if (!value || typeof value !== "object" || Array.isArray(value)) return undefined;
  return value as JsonObject;
}
