import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import {
  SERVER_HTTP_DEFINES,
  SERVER_HTTP_EXTERNAL_DEPENDENCIES,
  SERVER_HTTP_UNLOADED_EXTERNAL_DEPENDENCIES,
} from "../tsup.config.js";
import { loadBuiltinProviderConfig } from "../../../scripts/builtin-provider-config.mjs";

describe("server tsup config", () => {
  it("embeds the selected environment's Built-in config used by the HTTP entry", async () => {
    const expected = await loadBuiltinProviderConfig();
    expect(JSON.parse(SERVER_HTTP_DEFINES.__ESCODE_BUILTIN_PROVIDER_CONFIG_JSON__)).toBe(
      expected.content,
    );
    expect(JSON.parse(SERVER_HTTP_DEFINES.__ESCODE_ENV__)).toBe(expected.environment);
  });

  it("externalizes CommonJS multipart dependencies for the ESM HTTP dev bundle", () => {
    expect(SERVER_HTTP_EXTERNAL_DEPENDENCIES).toEqual(
      expect.arrayContaining([
        "axios",
        "form-data",
        "combined-stream",
        "proxy-from-env",
        "follow-redirects",
        "yazl",
        "node-forge",
        "yaml",
      ]),
    );
  });

  it("declares external runtime dependencies in the server package", () => {
    const packageJsonPath = resolve(import.meta.dirname, "../package.json");
    const packageJson = JSON.parse(readFileSync(packageJsonPath, "utf-8")) as {
      dependencies?: Record<string, string>;
    };

    // Bugfix: pnpm 不会把传递依赖暴露给 @escode/server；HTTP bundle 外置后必须从 server 自身解析。
    // 修复：串口依赖在 server 运行时从不加载（仅 Desktop 动态 import），不要求 server 声明，
    // 否则会逼 server 部署安装 native 串口包。其余外置依赖仍必须由 server 自身声明。
    const loaded = SERVER_HTTP_EXTERNAL_DEPENDENCIES.filter(
      (name) => !SERVER_HTTP_UNLOADED_EXTERNAL_DEPENDENCIES.includes(name),
    );
    expect(Object.keys(packageJson.dependencies ?? {})).toEqual(expect.arrayContaining(loaded));
  });
});
