import { readFile } from "node:fs/promises";
import { beforeAll, describe, expect, it, vi } from "vitest";
import type { JsonObject } from "@escode/model-option-map";
import {
  createRegistryModelConfig,
  createRegistryProviderConfig,
  parseProviderConfig,
  parseESCodeBuiltinModelConfigRules,
} from "@escode/provider";
import { ensureAnthropicThinkingBudget } from "../src/model/anthropic-thinking-budget.js";
import { AiSdkModelAdapter } from "../src/model/runner.js";

describe("Anthropic Messages thinking budget", () => {
  it("fills the spec budget derived from max_tokens", () => {
    expect(
      ensureAnthropicThinkingBudget({
        model: "fixture",
        max_tokens: 32000,
        thinking: { type: "enabled" },
        output_config: { effort: "high" },
      }),
    ).toEqual({
      model: "fixture",
      max_tokens: 32000,
      thinking: { type: "enabled", budget_tokens: 16000 },
      output_config: { effort: "high" },
    });
  });

  it("keeps the spec floor for small output caps and drops thinking below it", () => {
    expect(
      ensureAnthropicThinkingBudget({ max_tokens: 1500, thinking: { type: "enabled" } }),
    ).toEqual({ max_tokens: 1500, thinking: { type: "enabled", budget_tokens: 1024 } });
    expect(
      ensureAnthropicThinkingBudget({ max_tokens: 1024, thinking: { type: "enabled" } }),
    ).toEqual({ max_tokens: 1024 });
  });

  it("respects an explicit budget and leaves other thinking modes untouched", () => {
    const explicit = { max_tokens: 32000, thinking: { type: "enabled", budget_tokens: 2048 } };
    expect(ensureAnthropicThinkingBudget(explicit)).toBe(explicit);
    const untouched: JsonObject[] = [
      { max_tokens: 32000, thinking: { type: "adaptive" }, output_config: { effort: "high" } },
      { max_tokens: 32000, thinking: { type: "disabled" } },
      { max_tokens: 32000 },
      { thinking: { type: "enabled" } },
    ];
    for (const body of untouched) {
      expect(ensureAnthropicThinkingBudget(body)).toBe(body);
    }
  });
});

let rules: ReturnType<typeof parseESCodeBuiltinModelConfigRules>;
beforeAll(async () => {
  const release = JSON.parse(
    await readFile(
      new URL("../../../../../config/provider/escode-builtin.json", import.meta.url),
      "utf8",
    ),
  );
  rules = parseESCodeBuiltinModelConfigRules(release.config.modelConfigRules);
});

async function capture(modelId: string, reasoningLevel: string, maxOutputTokens: number) {
  const apiType = "anthropic-messages" as const;
  const config = createRegistryModelConfig(
    rules.resolve({ providerId: "custom-fixture", modelId, apiType, baseUrl: "https://vllm.invalid/v1" }),
  );
  const provider = createRegistryProviderConfig(
    parseProviderConfig({
      group: "standard-personal",
      access: { type: "api-key", apiKey: "fixture" },
      api: { type: apiType, baseUrl: "https://vllm.invalid/v1" },
    }),
  );
  if (!config.ok || !provider.ok) throw new Error("incomplete fixture");
  let body: Record<string, any> | undefined;
  vi.stubGlobal("fetch", async (url: RequestInfo | URL, init?: RequestInit) => {
    body = await new Request(url, init).json();
    return Response.json(
      { error: { type: "invalid_request_error", message: "captured" } },
      { status: 400 },
    );
  });
  try {
    const adapter = new AiSdkModelAdapter({ env: {}, retry: { maxAttempts: 1 } });
    const model = adapter.createModel({
      providerId: "custom-fixture" as never,
      modelId: modelId as never,
      providerConfig: provider.config,
      modelConfig: config.config,
      options: { maxOutputTokens, reasoningLevel },
    });
    await model.generateText({ messages: [{ role: "user", content: "hi" }], tools: [] } as never);
  } catch {
    // 正文已经在拦截层捕获。
  }
  vi.unstubAllGlobals();
  if (!body) throw new Error("request was not captured");
  return body;
}

describe("Anthropic Messages thinking budget on the wire", () => {
  it("sends a spec-valid thinking object for a private Anthropic-compatible model", async () => {
    const body = await capture("deepseek-v4.1-flash", "high", 32000);
    expect(body.max_tokens).toBe(32000);
    expect(body.thinking.type).toBe("enabled");
    expect(Number.isInteger(body.thinking.budget_tokens)).toBe(true);
    expect(body.thinking.budget_tokens).toBeGreaterThanOrEqual(1024);
    expect(body.thinking.budget_tokens).toBeLessThan(body.max_tokens);
  });

  it("omits thinking when the output cap cannot hold the spec floor", async () => {
    const body = await capture("deepseek-v4.1-flash", "high", 1024);
    expect(body.thinking).toBeUndefined();
  });

  it("keeps the disabled level without a budget", async () => {
    const body = await capture("deepseek-v4.1-flash", "disabled", 32000);
    expect(body.thinking).toEqual({ type: "disabled" });
  });
});
