import { describe, expect, it } from "vitest";
import {
  buildCliESCodeSourceHeaders,
  createRuntimeAiSdkModelExecutionConfig,
} from "../src/model-config.js";
import { createModelAdapter, type CreateModelAdapterOptions } from "../src/model-factory.js";

describe("createModelAdapter", () => {
  it("只接受执行基础设施配置，不接受 Provider Registry 投影", () => {
    expect(() => createModelAdapter({ env: {} } as CreateModelAdapterOptions)).toThrow(
      "requires executionConfig",
    );
    const adapter = createModelAdapter({
      env: {},
      executionConfig: createRuntimeAiSdkModelExecutionConfig({}),
    });
    expect(adapter).not.toHaveProperty("replaceRegistryConfig");
    expect(adapter).not.toHaveProperty("resolveConnection");
  });

  it("接受模型 IO 与流超时等 Adapter 自身选项", () => {
    expect(() =>
      createModelAdapter({
        env: {},
        executionConfig: createRuntimeAiSdkModelExecutionConfig({}),
        modelIoDir: "/tmp/escode-model-io-test",
        modelIoFullRetentionEnabled: true,
        streamIdleTimeoutMs: 42_000,
      }),
    ).not.toThrow();
  });
});

describe("createRuntimeAiSdkModelExecutionConfig", () => {
  it("只装配来源 Header、网络与环境基础设施", () => {
    const config = createRuntimeAiSdkModelExecutionConfig(
      { ESCODE_APP_VERSION: "3.9.0", ESCODE_ENV: "test" },
      {
        network: {
          caCertFile: "/tmp/test-ca.pem",
          httpProxy: "http://proxy.example.com",
          noProxy: "localhost",
        },
        sourceTitle: "electron",
      },
    );
    expect(config).toMatchObject({
      defaultHeaders: {
        "User-Agent": "ESCode/3.9.0",
        "X-ZCode-App-Version": "3.9.0",
        "X-Title": "ESCode@electron",
      },
      env: { ESCODE_APP_VERSION: "3.9.0", ESCODE_ENV: "test" },
      network: {
        caCertFile: "/tmp/test-ca.pem",
        httpProxy: "http://proxy.example.com",
        noProxy: "localhost",
      },
    });
    expect(config).not.toHaveProperty("providers");
    expect(config).not.toHaveProperty("defaultProviderId");
    expect(config).not.toHaveProperty("endpointRoutingPort");
  });

  it("过滤空网络配置", () => {
    expect(createRuntimeAiSdkModelExecutionConfig({}, { network: {} })).not.toHaveProperty(
      "network",
    );
  });
});

describe("buildCliESCodeSourceHeaders", () => {
  it("生成运行时来源 Header，不根据模型或 Provider 推断", () => {
    expect(
      buildCliESCodeSourceHeaders(
        { ESCODE_APP_VERSION: "3.9.0", ESCODE_ENV: "production" },
        { sourceTitle: "cli" },
      ),
    ).toMatchObject({
      "User-Agent": "ESCode/3.9.0",
      "X-Title": "ESCode@cli",
      "X-ZCode-App-Version": "3.9.0",
    });
  });
});
