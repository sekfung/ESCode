import { describe, expect, it } from "vitest";
import {
  DEFAULT_BIGMODEL_API_ORIGIN,
  DEFAULT_ZAI_BUSINESS_BASE_URL,
  DEFAULT_ZAI_OAUTH_CLIENT_ID,
  DEFAULT_ZAI_OAUTH_ORIGIN,
  DEFAULT_ESCODE_ENDPOINT_ORIGIN,
  TEST_BIGMODEL_API_ORIGIN,
  TEST_ZAI_BUSINESS_BASE_URL,
  TEST_ZAI_OAUTH_CLIENT_ID,
  TEST_ZAI_OAUTH_ORIGIN,
  TEST_ESCODE_ENDPOINT_ORIGIN,
  buildBigModelApiUrl,
  buildRuntimeESCodeApiUrl,
  buildRuntimeZaiBusinessUrl,
  buildRuntimeZaiOAuthUrl,
  buildRuntimeESCodeEndpointUrls,
  buildESCodeEndpointUrls,
  normalizeESCodeEndpointOrigin,
  resolveBigModelApiOrigin,
  resolveRuntimeProductEndpointConfig,
  resolveZaiBusinessBaseUrl,
  resolveZaiOAuthClientId,
  resolveZaiOAuthOrigin,
  resolveESCodeEndpointOrigin,
} from "../src/escodeEndpoint.js";
import { appSettingsPatchSchema, appSettingsSchema } from "../src/validationAppSettings.js";

describe("escodeEndpoint", () => {
  it("defaults to the scoped origin in every env without env base url", () => {
    expect(resolveESCodeEndpointOrigin({ env: "test" })).toBe(TEST_ESCODE_ENDPOINT_ORIGIN);
    expect(resolveESCodeEndpointOrigin({ env: "production" })).toBe(DEFAULT_ESCODE_ENDPOINT_ORIGIN);
  });

  it("uses ESCODE_BASE_URL style env base as the default endpoint", () => {
    expect(
      resolveESCodeEndpointOrigin({
        env: "test",
        envBaseOrigin: "https://zcode.z.ai/api/v1",
      }),
    ).toBe(TEST_ESCODE_ENDPOINT_ORIGIN);
    expect(
      resolveESCodeEndpointOrigin({
        env: "production",
        envBaseOrigin: "https://zcode.z.ai/api/v1",
      }),
    ).toBe(DEFAULT_ESCODE_ENDPOINT_ORIGIN);
  });

  it("keeps the non-production settings override above env base url", () => {
    expect(
      resolveESCodeEndpointOrigin({
        env: "test",
        envBaseOrigin: "https://zcode.z.ai",
        overrideOrigin: "http://localhost:3030/api/v1",
      }),
    ).toBe("http://localhost:3030");
  });

  it("uses a non-production override after normalizing to origin", () => {
    expect(
      resolveESCodeEndpointOrigin({
        env: "test",
        overrideOrigin: "https://zcode.z.ai/api/v1/ignored?x=1",
      }),
    ).toBe(TEST_ESCODE_ENDPOINT_ORIGIN);
  });

  it("ignores overrides in production", () => {
    expect(
      resolveESCodeEndpointOrigin({
        env: "production",
        envBaseOrigin: "https://zcode.z.ai",
        overrideOrigin: "https://zcode.z.ai",
      }),
    ).toBe(DEFAULT_ESCODE_ENDPOINT_ORIGIN);
  });

  it("derives http and websocket urls from the same origin", () => {
    expect(buildESCodeEndpointUrls("http://localhost:3030")).toEqual({
      origin: "http://localhost:3030",
      apiBaseUrl: "http://localhost:3030/api/v1",
      escodePlanOpenAiBaseUrl: "http://localhost:3030/api/v1/zcode-plan",
      escodePlanAnthropicBaseUrl: "http://localhost:3030/api/v1/zcode-plan/anthropic",
    });
  });

  it("derives runtime zcode-plan urls from ESCODE_ENV", () => {
    expect(buildRuntimeESCodeEndpointUrls({ ESCODE_ENV: "test" }).escodePlanOpenAiBaseUrl).toBe(
      "https://zcode.z.ai/api/v1/zcode-plan",
    );
    expect(
      buildRuntimeESCodeEndpointUrls({ ESCODE_ENV: "production" }).escodePlanOpenAiBaseUrl,
    ).toBe("https://zcode.z.ai/api/v1/zcode-plan");
    expect(buildRuntimeESCodeEndpointUrls({}).escodePlanOpenAiBaseUrl).toBe(
      "https://zcode.z.ai/api/v1/zcode-plan",
    );
    expect(
      buildRuntimeESCodeEndpointUrls({
        ESCODE_ENV: "test",
        ESCODE_BASE_URL: "https://zcode.z.ai/ignored/path",
      }).escodePlanAnthropicBaseUrl,
    ).toBe("https://zcode.z.ai/api/v1/zcode-plan/anthropic");
  });

  it("derives BigModel API origin from ESCODE_ENV", () => {
    expect(resolveBigModelApiOrigin({ ESCODE_ENV: "production" })).toBe("https://bigmodel.cn");
    expect(resolveBigModelApiOrigin({ ESCODE_ENV: "test" })).toBe("https://bigmodel.cn");
    expect(buildBigModelApiUrl({ ESCODE_ENV: "test" }, "/api/biz/subscription/list")).toBe(
      "https://bigmodel.cn/api/biz/subscription/list",
    );
  });

  it("resolves all product endpoints for the test product environment", () => {
    expect(resolveRuntimeProductEndpointConfig({ ESCODE_ENV: "test" })).toMatchObject({
      escodeEnv: "test",
      escodeEndpointOrigin: TEST_ESCODE_ENDPOINT_ORIGIN,
      zaiOAuthOrigin: TEST_ZAI_OAUTH_ORIGIN,
      zaiBusinessBaseUrl: TEST_ZAI_BUSINESS_BASE_URL,
      zaiOAuthClientId: TEST_ZAI_OAUTH_CLIENT_ID,
      bigModelApiOrigin: TEST_BIGMODEL_API_ORIGIN,
    });
  });

  it("resolves all product endpoints for the production product environment", () => {
    expect(resolveRuntimeProductEndpointConfig({ ESCODE_ENV: "production" })).toMatchObject({
      escodeEnv: "production",
      escodeEndpointOrigin: DEFAULT_ESCODE_ENDPOINT_ORIGIN,
      zaiOAuthOrigin: DEFAULT_ZAI_OAUTH_ORIGIN,
      zaiBusinessBaseUrl: DEFAULT_ZAI_BUSINESS_BASE_URL,
      zaiOAuthClientId: DEFAULT_ZAI_OAUTH_CLIENT_ID,
      bigModelApiOrigin: DEFAULT_BIGMODEL_API_ORIGIN,
    });
  });

  it("builds product endpoint URLs from the selected environment", () => {
    expect(buildRuntimeESCodeApiUrl({ ESCODE_ENV: "test" }, "/api/v1/oauth/token")).toBe(
      "https://zcode.z.ai/api/v1/oauth/token",
    );
    expect(buildRuntimeZaiOAuthUrl({ ESCODE_ENV: "test" }, "/api/oauth/authorize")).toBe(
      "https://chat.z.ai/api/oauth/authorize",
    );
    expect(buildRuntimeZaiBusinessUrl({ ESCODE_ENV: "test" }, "/api/auth/z/login")).toBe(
      "https://api.z.ai/api/auth/z/login",
    );
    expect(buildBigModelApiUrl({ ESCODE_ENV: "test" }, "/login")).toBe("https://bigmodel.cn/login");
  });

  it("keeps environment-scoped endpoint overrides centralized", () => {
    const env = {
      ESCODE_ENV: "test",
      ESCODE_TEST_BASE_URL: "https://test-escode.example.com/path",
      ZAI_TEST_OAUTH_ORIGIN: "https://test-oauth.example.com/oauth",
      ZAI_TEST_BUSINESS_BASE_URL: "https://test-business.example.com/api",
      ZAI_TEST_OAUTH_CLIENT_ID: "client_test_override",
      BIGMODEL_TEST_API_BASE_URL: "https://test-bigmodel.example.com/base",
    };

    expect(resolveRuntimeProductEndpointConfig(env)).toMatchObject({
      escodeEndpointOrigin: "https://test-escode.example.com",
      zaiOAuthOrigin: "https://test-oauth.example.com",
      zaiBusinessBaseUrl: "https://test-business.example.com",
      zaiOAuthClientId: "client_test_override",
      bigModelApiOrigin: "https://test-bigmodel.example.com",
    });
  });

  it("keeps unscoped endpoint overrides above scoped endpoint overrides", () => {
    const env = {
      ESCODE_ENV: "test",
      ZAI_OAUTH_ORIGIN: "https://oauth-global.example.com",
      ZAI_TEST_OAUTH_ORIGIN: "https://oauth-test.example.com",
      ZAI_BUSINESS_BASE_URL: "https://business-global.example.com",
      ZAI_TEST_BUSINESS_BASE_URL: "https://business-test.example.com",
      ZAI_OAUTH_CLIENT_ID: "client_global",
      ZAI_TEST_OAUTH_CLIENT_ID: "client_test",
      BIGMODEL_API_BASE_URL: "https://bigmodel-global.example.com",
      BIGMODEL_TEST_API_BASE_URL: "https://bigmodel-test.example.com",
    };

    expect(resolveZaiOAuthOrigin(env)).toBe("https://oauth-global.example.com");
    expect(resolveZaiBusinessBaseUrl(env)).toBe("https://business-global.example.com");
    expect(resolveZaiOAuthClientId(env)).toBe("client_global");
    expect(resolveBigModelApiOrigin(env)).toBe("https://bigmodel-global.example.com");
  });

  it("ignores blank runtime endpoint overrides", () => {
    const env = {
      ESCODE_ENV: "test",
      ESCODE_BASE_URL: " ",
      ESCODE_TEST_BASE_URL: "\t",
      ZAI_OAUTH_ORIGIN: "",
      ZAI_TEST_OAUTH_ORIGIN: " ",
      ZAI_BUSINESS_BASE_URL: " ",
      ZAI_TEST_BUSINESS_BASE_URL: "",
      ZAI_OAUTH_CLIENT_ID: "",
      ZAI_TEST_OAUTH_CLIENT_ID: " ",
      BIGMODEL_API_BASE_URL: " ",
      BIGMODEL_TEST_API_BASE_URL: "",
    };

    expect(resolveRuntimeProductEndpointConfig(env)).toMatchObject({
      escodeEndpointOrigin: TEST_ESCODE_ENDPOINT_ORIGIN,
      zaiOAuthOrigin: TEST_ZAI_OAUTH_ORIGIN,
      zaiBusinessBaseUrl: TEST_ZAI_BUSINESS_BASE_URL,
      zaiOAuthClientId: TEST_ZAI_OAUTH_CLIENT_ID,
      bigModelApiOrigin: TEST_BIGMODEL_API_ORIGIN,
    });
  });

  it("rejects non-http endpoint origins", () => {
    expect(() => normalizeESCodeEndpointOrigin("file:///tmp/escode")).toThrow(/http or https/);
  });

  it("persists optional endpoint override in app settings", () => {
    expect(
      appSettingsSchema.parse({ escodeEndpointOrigin: "https://zcode.z.ai/path" })
        .escodeEndpointOrigin,
    ).toBe(TEST_ESCODE_ENDPOINT_ORIGIN);
    expect(
      appSettingsPatchSchema.parse({ escodeEndpointOrigin: "http://localhost:3030/foo" })
        .escodeEndpointOrigin,
    ).toBe("http://localhost:3030");
  });
});
