import { describe, expect, it } from "vitest";
import {
  ESCODE_TOOL_ENV_PASSTHROUGH_ENV_KEY,
  ESCODE_REMOTE_HTTP_PROXY_ENV_KEY,
  ESCODE_REMOTE_NO_PROXY_ENV_KEY,
  ESCODE_REMOTE_RUNTIME_NETWORK_AUTHORITY_ENV_KEY,
  buildESCodeToolEnvPassthroughEnv,
  getCapturedESCodeCuaBrokerCredentials,
  isCuaDevModeRequested,
  isESCodeCuaInternalFeatureEnabled,
  readESCodeToolEnvPassthroughEnv,
  resetCapturedESCodeCuaBrokerCredentialsForTest,
  sanitizeESCodeRuntimeEnv,
} from "../src/runtimeEnv.js";

describe("runtimeEnv", () => {
  it("enables internal CUA by default while honoring explicit false values", () => {
    expect(isESCodeCuaInternalFeatureEnabled({})).toBe(true);
    expect(isESCodeCuaInternalFeatureEnabled({ ESCODE_CUA_PRODUCT_HELPER: "0" })).toBe(false);
    expect(isESCodeCuaInternalFeatureEnabled({ ESCODE_CUA_PRODUCT_HELPER: "false" })).toBe(false);
    expect(isESCodeCuaInternalFeatureEnabled({ ESCODE_CUA_PRODUCT_HELPER: "1" })).toBe(true);
    expect(isESCodeCuaInternalFeatureEnabled({ ESCODE_CUA_PRODUCT_HELPER: " TRUE " })).toBe(true);
    expect(isESCodeCuaInternalFeatureEnabled({ ESCODE_CUA_PRODUCT_HELPER: "on" })).toBe(true);
  });

  it("parses explicit truthy values for ESCODE_CUA_DEV_MODE", () => {
    // isCuaDevModeRequested: only explicit truthy values (case-insensitive, trimmed).
    expect(isCuaDevModeRequested({})).toBe(false);
    expect(isCuaDevModeRequested({ ESCODE_CUA_DEV_MODE: "0" })).toBe(false);
    expect(isCuaDevModeRequested({ ESCODE_CUA_DEV_MODE: "false" })).toBe(false);
    expect(isCuaDevModeRequested({ ESCODE_CUA_DEV_MODE: "1" })).toBe(true);
    expect(isCuaDevModeRequested({ ESCODE_CUA_DEV_MODE: " ON " })).toBe(true);
    expect(isCuaDevModeRequested({ ESCODE_CUA_DEV_MODE: "true" })).toBe(true);
  });

  it("sanitizes runtime env while sealing user network env for tools", () => {
    const sourceEnv = {
      HTTP_PROXY: "http://proxy.example:8080",
      NODE_ENV: "development",
      NODE_EXTRA_CA_CERTS: "/tmp/root-ca.pem",
      PATH: "/usr/bin:/bin",
      npm_config_proxy: "http://npm-proxy.example:8080",
    };

    const sanitized = sanitizeESCodeRuntimeEnv(sourceEnv);
    const passthroughEnv = buildESCodeToolEnvPassthroughEnv(sourceEnv);
    const captured = readESCodeToolEnvPassthroughEnv(passthroughEnv);

    expect(sanitized).toEqual({ PATH: "/usr/bin:/bin" });
    expect(captured).toEqual({
      HTTP_PROXY: "http://proxy.example:8080",
      NODE_EXTRA_CA_CERTS: "/tmp/root-ca.pem",
      npm_config_proxy: "http://npm-proxy.example:8080",
    });
  });

  it("ignores invalid passthrough JSON and non-network keys", () => {
    const captured = readESCodeToolEnvPassthroughEnv({
      [ESCODE_TOOL_ENV_PASSTHROUGH_ENV_KEY]: JSON.stringify({
        HTTP_PROXY: "http://proxy.example:8080",
        NODE_ENV: "production",
        PATH: "/tmp/bin",
      }),
    });

    expect(captured).toEqual({
      HTTP_PROXY: "http://proxy.example:8080",
    });
    expect(
      readESCodeToolEnvPassthroughEnv({
        [ESCODE_TOOL_ENV_PASSTHROUGH_ENV_KEY]: "not-json",
      }),
    ).toEqual({});
  });

  it("strips the CUA broker socket/authority/refresh marker from child env and tool passthrough", () => {
    // Confused-deputy 防护：broker bearer 凭据只该定向注入目标 escode-cua server，绝不能随全局 env
    // 泄漏给其它 MCP / Bash / tool 子进程，也不能经 tool-env-passthrough 恢复。
    const sourceEnv = {
      PATH: "/usr/bin:/bin",
      ESCODE_CUA_PERMISSION_BROKER_SOCKET: "/tmp/escode-cua-me/broker-abc.sock",
      ESCODE_CUA_PERMISSION_BROKER_REFRESH_MARKER:
        "/tmp/escode-cua-me/broker-abc.sock.permission-refresh.json",
    };
    const sanitized = sanitizeESCodeRuntimeEnv(sourceEnv);
    expect(sanitized).toEqual({ PATH: "/usr/bin:/bin" });

    const captured = readESCodeToolEnvPassthroughEnv(buildESCodeToolEnvPassthroughEnv(sourceEnv));
    expect(captured).not.toHaveProperty("ESCODE_CUA_PERMISSION_BROKER_SOCKET");
    expect(captured).not.toHaveProperty("ESCODE_CUA_PERMISSION_BROKER_REFRESH_MARKER");
  });

  it("strips desktop-to-remote network handoff values after stdio bootstrap", () => {
    const sourceEnv = {
      PATH: "/usr/bin:/bin",
      [ESCODE_REMOTE_RUNTIME_NETWORK_AUTHORITY_ENV_KEY]: "1",
      [ESCODE_REMOTE_HTTP_PROXY_ENV_KEY]: "http://172.21.240.1:7890",
      [ESCODE_REMOTE_NO_PROXY_ENV_KEY]: "localhost,127.0.0.1",
    };

    expect(sanitizeESCodeRuntimeEnv(sourceEnv)).toEqual({ PATH: "/usr/bin:/bin" });
    expect(readESCodeToolEnvPassthroughEnv(buildESCodeToolEnvPassthroughEnv(sourceEnv))).toEqual(
      {},
    );
  });

  it("captures broker credentials as one atomic pair and clears stale state on partial rotation", () => {
    resetCapturedESCodeCuaBrokerCredentialsForTest();
    try {
      sanitizeESCodeRuntimeEnv({
        ESCODE_CUA_PERMISSION_BROKER_SOCKET: "/tmp/broker-a.sock",
        ESCODE_CUA_PLUGIN_AUTHORITY: "authority-a",
      });
      expect(getCapturedESCodeCuaBrokerCredentials()).toEqual({
        socket: "/tmp/broker-a.sock",
        pluginAuthority: "authority-a",
      });

      sanitizeESCodeRuntimeEnv({
        ESCODE_CUA_PERMISSION_BROKER_SOCKET: "/tmp/broker-b.sock",
      });
      expect(getCapturedESCodeCuaBrokerCredentials()).toEqual({
        socket: undefined,
        pluginAuthority: undefined,
      });

      sanitizeESCodeRuntimeEnv({
        ESCODE_CUA_PERMISSION_BROKER_SOCKET: "/tmp/broker-b.sock",
        ESCODE_CUA_PLUGIN_AUTHORITY: "authority-b",
      });
      expect(getCapturedESCodeCuaBrokerCredentials()).toEqual({
        socket: "/tmp/broker-b.sock",
        pluginAuthority: "authority-b",
      });
    } finally {
      resetCapturedESCodeCuaBrokerCredentialsForTest();
    }
  });
});
