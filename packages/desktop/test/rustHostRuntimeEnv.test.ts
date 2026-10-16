import { describe, expect, it } from "vitest";
import { chmod, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { buildRustHostProviderEnv } from "../src/main/desktopRuntimeEnv.js";
import { startRustHost } from "../src/main/rustHostBridge.js";
import { createRustHostManager } from "../src/main/rustHostManager.js";

// 审计 A16：Main 合成的嵌入运行时环境必须随 `rpcListen` 原样交给 Host。这里用一个只做握手的假 Host，
// 断言线上的 `rpcListen` 消息（camelCase `runtimeEnv`），Rust 侧的解析与合成由 host-control / control_tests 覆盖。

async function fakeHost(): Promise<{ dir: string; binary: string; captured: string }> {
  const dir = await mkdtemp(join(tmpdir(), "escode-rust-host-env-"));
  const binary = join(dir, "fake-host.sh");
  const captured = join(dir, "rpc-listen.json");
  await writeFile(
    binary,
    `#!/bin/sh
printf '{"kind":"hostReady"}\\n'
IFS= read -r line
printf '%s' "$line" > "$FAKE_HOST_CAPTURE"
printf '{"kind":"rpcListening","addr":"127.0.0.1:1","secret":"s"}\\n'
cat > /dev/null
`,
  );
  await chmod(binary, 0o755);
  return { dir, binary, captured };
}

describe.skipIf(process.platform === "win32")("rust host runtimeEnv handoff", () => {
  it("sends Main's runtime env on rpcListen and omits it when absent", async () => {
    const { dir, binary, captured } = await fakeHost();
    try {
      const withEnv = await startRustHost({
        binary,
        home: dir,
        runtimeEnv: { ESCODE_DATA_BASE_DIR: "/data", ESCODE_APP_VERSION: "1.2.3" },
        env: { FAKE_HOST_CAPTURE: captured },
      });
      await withEnv.stop();
      expect(JSON.parse(await readFile(captured, "utf8"))).toMatchObject({
        kind: "rpcListen",
        home: dir,
        runtimeEnv: { ESCODE_DATA_BASE_DIR: "/data", ESCODE_APP_VERSION: "1.2.3" },
      });

      const without = await startRustHost({
        binary,
        home: dir,
        env: { FAKE_HOST_CAPTURE: captured },
      });
      await without.stop();
      expect(JSON.parse(await readFile(captured, "utf8"))).not.toHaveProperty("runtimeEnv");
    } finally {
      await rm(dir, { recursive: true, force: true });
    }
  });

  it("evaluates the manager's runtimeEnv lazily at first start", async () => {
    const { dir, binary, captured } = await fakeHost();
    const saved = process.env.FAKE_HOST_CAPTURE;
    process.env.FAKE_HOST_CAPTURE = captured;
    let calls = 0;
    try {
      const manager = createRustHostManager({
        binary,
        home: dir,
        enabled: true,
        runtimeEnv: () => {
          calls += 1;
          return { ESCODE_RUNTIME_ENV: "test" };
        },
      });
      expect(calls).toBe(0);
      await manager.ensureStarted();
      await manager.ensureStarted();
      expect(calls).toBe(1);
      await manager.stop();
      expect(JSON.parse(await readFile(captured, "utf8")).runtimeEnv).toEqual({
        ESCODE_RUNTIME_ENV: "test",
      });
    } finally {
      if (saved === undefined) delete process.env.FAKE_HOST_CAPTURE;
      else process.env.FAKE_HOST_CAPTURE = saved;
      await rm(dir, { recursive: true, force: true });
    }
  });
});

describe("buildRustHostProviderEnv", () => {
  it("derives the personal file from the config dir and forwards the Active/LKG locator", () => {
    expect(
      buildRustHostProviderEnv({
        builtinFilePath: "/res/escode-builtin.json",
        appConfigDir: "/data/.escode/v2",
        appVersion: "1.2.3",
        endpointOrigin: "https://api.example.com",
      }),
    ).toEqual({
      ESCODE_BUILTIN_PROVIDER_CONFIG_FILE: "/res/escode-builtin.json",
      ESCODE_PERSONAL_PROVIDER_CONFIG_FILE: join("/data/.escode/v2", "provider_config.json"),
      ESCODE_BUILTIN_CACHE_ROOT: "/data/.escode/v2",
      ESCODE_APP_VERSION: "1.2.3",
      ESCODE_ENDPOINT_ORIGIN: "https://api.example.com",
    });
  });
});
