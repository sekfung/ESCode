import { describe, expect, it } from "vitest";
import { escodeProviderRuntimeHeadersResponseSchema } from "@escode/shared";

describe("request-scoped provider runtime headers response", () => {
  it("preserves the paired PAT and ID across the private protocol and rejects empty IDs", () => {
    const response = { headersApplied: true, requestAuth: { apiKey: "pat", apiKeyId: "key-id" } };
    expect(escodeProviderRuntimeHeadersResponseSchema.parse(response)).toEqual(response);
    expect(
      escodeProviderRuntimeHeadersResponseSchema.safeParse({
        ...response,
        requestAuth: { apiKeyId: " " },
      }).success,
    ).toBe(false);
  });
  it("requires headers on success instead of acknowledging a shared config mutation", () => {
    expect(
      escodeProviderRuntimeHeadersResponseSchema.safeParse({ headersApplied: true }).success,
    ).toBe(false);
    expect(
      escodeProviderRuntimeHeadersResponseSchema.parse({
        headersApplied: true,
        requestAuth: {
          headers: {
            "X-Request-Scoped-Param": "request-token",
            "X-Request-Scoped-Region": "cn",
          },
        },
      }).headersApplied,
    ).toBe(true);
  });
  it("preserves setup errors and rejects unexpected fields", () => {
    // Agent 协议不接受旧裸 headers 成功格式；UI 提供的请求级输入由 Host 另行合成。
    expect(
      escodeProviderRuntimeHeadersResponseSchema.safeParse({
        headersApplied: true,
        runtimeProviderHeaders: { token: "legacy" },
      }).success,
    ).toBe(false);
    expect(
      escodeProviderRuntimeHeadersResponseSchema.parse({
        headersApplied: false,
        errorMessage: "SDK initialization timeout",
      }),
    ).toEqual({ headersApplied: false, errorMessage: "SDK initialization timeout" });
    expect(
      escodeProviderRuntimeHeadersResponseSchema.safeParse({
        headersApplied: false,
        runtimeProviderHeaders: { token: "unexpected" },
      }).success,
    ).toBe(false);
  });
});
