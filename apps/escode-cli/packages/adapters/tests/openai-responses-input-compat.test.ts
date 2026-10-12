import { streamText } from "ai";
import { describe, expect, it } from "vitest";
import type { ModelInputMessage } from "@escode/contracts";
import { normalizeOpenAIResponsesInput } from "../src/model/openai-responses-input-compat.js";
import { createStreamTextOptions } from "../src/model/runner-options.js";
import { TestProviderConfigFixture } from "./test-provider-config.js";
import { createTestModelProperties } from "./test-model-format.js";

describe("OpenAI Responses input item protocol", () => {
  it("collapses a single assistant output_text part into string content", () => {
    expect(
      normalizeOpenAIResponsesInput({
        input: [
          {
            role: "assistant",
            phase: "final_answer",
            content: [{ type: "output_text", text: "answer" }],
          },
        ],
      }),
    ).toEqual({
      input: [{ role: "assistant", phase: "final_answer", content: "answer" }],
    });
  });

  it("rewrites assistant text parts to input_text when the message keeps a part list", () => {
    expect(
      normalizeOpenAIResponsesInput({
        input: [
          {
            role: "assistant",
            content: [
              { type: "output_text", text: "first" },
              { type: "input_image", image_url: "data:image/png;base64,aW1n" },
              { type: "output_text", text: "second" },
            ],
          },
        ],
      }),
    ).toEqual({
      input: [
        {
          role: "assistant",
          content: [
            { type: "input_text", text: "first" },
            { type: "input_image", image_url: "data:image/png;base64,aW1n" },
            { type: "input_text", text: "second" },
          ],
        },
      ],
    });
  });

  it("leaves output messages, function calls and function results untouched", () => {
    const body = {
      input: [
        { role: "system", content: "system text" },
        { role: "user", content: [{ type: "input_text", text: "hi" }] },
        {
          type: "message",
          id: "msg_fixture",
          status: "completed",
          role: "assistant",
          content: [{ type: "output_text", text: "stored output", annotations: [] }],
        },
        { type: "function_call", call_id: "call_1", name: "Read", arguments: "{}" },
        { type: "function_call_output", call_id: "call_1", output: "file body" },
        {
          type: "reasoning",
          id: "rs_fixture",
          summary: [],
          encrypted_content: "opaque-fixture",
        },
      ],
    };

    expect(normalizeOpenAIResponsesInput(body)).toBeUndefined();
  });

  it("still rewrites assistant text parts carrying an item id without output status", () => {
    expect(
      normalizeOpenAIResponsesInput({
        input: [
          {
            role: "assistant",
            id: "msg_stored",
            content: [
              { type: "output_text", text: "partial" },
              { type: "output_text", text: "output" },
            ],
          },
        ],
      }),
    ).toEqual({
      input: [
        {
          role: "assistant",
          id: "msg_stored",
          content: [
            { type: "input_text", text: "partial" },
            { type: "input_text", text: "output" },
          ],
        },
      ],
    });
  });

  it("ignores bodies without an input item list or without assistant text parts", () => {
    expect(normalizeOpenAIResponsesInput({ model: "fixture" })).toBeUndefined();
    expect(normalizeOpenAIResponsesInput({ input: "plain prompt" })).toBeUndefined();
    expect(
      normalizeOpenAIResponsesInput({
        input: [{ role: "assistant", content: "already a string" }],
      }),
    ).toBeUndefined();
  });

  it("keeps assistant history free of output_text on the wire", async () => {
    const bodies: Record<string, any>[] = [];
    const fixture = new TestProviderConfigFixture(
      {
        env: {},
        providers: {
          "api-responses": { kind: "openai", apiKey: "fake-key", baseURL: "https://probe.invalid/v1" },
        },
      },
      {
        // 离线拦截请求，绝不回落真实网络；只捕获发出去的正文。
        transport: async (input, init) => {
          bodies.push((await new Request(input, init).json()) as Record<string, any>);
          return new Response(
            JSON.stringify({
              error: { type: "invalid_request_error", message: "captured" },
            }),
            { status: 400, headers: { "content-type": "application/json" } },
          );
        },
      },
    );
    const messages: ModelInputMessage[] = [
      { role: "system", content: "You are ESCode." },
      { role: "user", content: "read the file" },
      {
        role: "assistant",
        content: "Let me read it.",
        toolCalls: [{ id: "call_1", name: "Read", input: { path: "a.md" } }],
      },
      { role: "tool", toolCallId: "call_1", toolName: "Read", content: "file body" },
      { role: "user", content: "continue" },
    ];
    const resolved = fixture.resolve(
      { providerId: "api-responses", modelId: "responses-model" },
      { maxOutputTokens: 64, reasoningLevel: "disabled" },
    );

    const result = streamText(
      createStreamTextOptions({
        includeModelIO: false,
        resolved: { ...resolved, properties: createTestModelProperties() },
        request: { messages },
        statusContext: {
          providerId: "api-responses",
          modelId: "responses-model",
          providerKind: resolved.providerKind,
          requestId: "input-compat-wire",
        } as never,
      }),
    );
    for await (const _event of result.fullStream) {
      // 只驱动请求本身，捕获的正文就是断言对象。
    }

    expect(bodies).toHaveLength(1);
    const input = bodies[0]!.input as any[];
    expect(input).toContainEqual({ role: "assistant", content: "Let me read it." });
    expect(input).toContainEqual({
      type: "function_call",
      call_id: "call_1",
      name: "Read",
      arguments: '{"path":"a.md"}',
    });
    expect(input).toContainEqual({
      type: "function_call_output",
      call_id: "call_1",
      output: "file body",
    });
    expect(JSON.stringify(input)).not.toContain("output_text");
  });
});
