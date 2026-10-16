import { describe, expect, it } from "vitest";
import { existsSync } from "node:fs";
import { chmod, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { buildRustHostClientEnv, buildRustHostProviderEnv } from "../src/main/desktopRuntimeEnv.js";
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
  it("derives the personal file from the config dir and forwards the cache/endpoint inputs", () => {
    expect(
      buildRustHostProviderEnv({
        builtinFilePath: "/res/escode-builtin.json",
        appConfigDir: "/data/.escode/v2",
        appVersion: "1.2.3",
        escodeEnv: "test",
        endpointBaseOrigin: "https://api.example.com",
      }),
    ).toEqual({
      ESCODE_BUILTIN_PROVIDER_CONFIG_FILE: "/res/escode-builtin.json",
      ESCODE_PERSONAL_PROVIDER_CONFIG_FILE: join("/data/.escode/v2", "provider_config.json"),
      ESCODE_BUILTIN_CACHE_ROOT: "/data/.escode/v2",
      ESCODE_APP_VERSION: "1.2.3",
      ESCODE_ENV: "test",
      ESCODE_ENDPOINT_ORIGIN: "https://api.example.com",
      ESCODE_BUILTIN_REMOTE_REFRESH: "1",
    });
  });

  it("omits the endpoint base when the environment does not define one", () => {
    const env = buildRustHostProviderEnv({
      builtinFilePath: "/b.json",
      appConfigDir: "/cfg",
      appVersion: "1.0.0",
      escodeEnv: "production",
    });
    expect(env).not.toHaveProperty("ESCODE_ENDPOINT_ORIGIN");
    expect(env.ESCODE_ENV).toBe("production");
  });
});

describe("buildRustHostClientEnv", () => {
  it("forwards the facts the Host cannot derive and omits missing ones", () => {
    expect(
      buildRustHostClientEnv({ language: "zh-CN", timezone: "Asia/Shanghai", osVersion: "#1 SMP" }),
    ).toEqual({
      ESCODE_CLIENT_LANGUAGE: "zh-CN",
      ESCODE_CLIENT_TIMEZONE: "Asia/Shanghai",
      ESCODE_OS_VERSION: "#1 SMP",
    });
    expect(buildRustHostClientEnv({ language: "en-US" })).toEqual({
      ESCODE_CLIENT_LANGUAGE: "en-US",
    });
  });

  it("derives defaults from Intl and the OS when called without facts", () => {
    const env = buildRustHostClientEnv();
    expect(env.ESCODE_CLIENT_LANGUAGE).toBe(Intl.DateTimeFormat().resolvedOptions().locale);
    expect(env.ESCODE_CLIENT_TIMEZONE).toBe(Intl.DateTimeFormat().resolvedOptions().timeZone);
  });
});

// docs/specs/rust-u2-attachments.md §5：Main 作为本地媒体路径授权的所有者，应答 Host 的控制帧。
describe.skipIf(process.platform === "win32")(
  "rust host local media authorization (Main side)",
  () => {
    async function authorizingHost(paths: string[]) {
      const dir = await mkdtemp(join(tmpdir(), "escode-rust-host-authz-"));
      const binary = join(dir, "fake-host.sh");
      const listen = join(dir, "rpc-listen.json");
      const replies = join(dir, "replies.ndjson");
      const requests = paths
        .map(
          (path, index) =>
            `printf '{"kind":"localMediaPreviewAuthorize","requestId":"r${index}","path":%s}\\n' '${JSON.stringify(path)}'\nIFS= read -r reply\nprintf '%s\\n' "$reply" >> "$FAKE_HOST_REPLIES"`,
        )
        .join("\n");
      await writeFile(
        binary,
        `#!/bin/sh
printf '{"kind":"hostReady"}\\n'
IFS= read -r line
printf '%s' "$line" > "$FAKE_HOST_CAPTURE"
printf '{"kind":"rpcListening","addr":"127.0.0.1:1","secret":"s"}\\n'
${requests}
cat > /dev/null
`,
      );
      await chmod(binary, 0o755);
      return { dir, binary, listen, replies };
    }

    it("declares the capability, authorizes through Main's registry and replies per request", async () => {
      const { dir, binary, listen, replies } = await authorizingHost(["/a b.mp4", "/denied.mp3"]);
      const seen: string[] = [];
      try {
        const handle = await startRustHost({
          binary,
          home: dir,
          env: { FAKE_HOST_CAPTURE: listen, FAKE_HOST_REPLIES: replies },
          authorizeLocalMediaPreviewPath: async (path) => {
            seen.push(path);
            if (path.includes("denied")) throw new Error("path not authorized");
            return `/canonical${path}`;
          },
        });
        // 假 Host 在 rpcListening 之后才发请求；等两条应答都落盘。
        for (let i = 0; i < 100 && !existsSync(replies); i += 1)
          await new Promise((r) => setTimeout(r, 20));
        await handle.stop();
        expect(JSON.parse(await readFile(listen, "utf8"))).toMatchObject({
          kind: "rpcListen",
          localMediaPreview: true,
        });
        expect(seen).toEqual(["/a b.mp4", "/denied.mp3"]);
        const lines = (await readFile(replies, "utf8"))
          .trim()
          .split("\n")
          .map((l) => JSON.parse(l));
        expect(lines).toEqual([
          {
            kind: "localMediaPreviewAuthorized",
            requestId: "r0",
            ok: true,
            path: "/canonical/a b.mp4",
          },
          {
            kind: "localMediaPreviewAuthorized",
            requestId: "r1",
            ok: false,
            error: "path not authorized",
          },
        ]);
      } finally {
        await rm(dir, { recursive: true, force: true });
      }
    });

    it("does not declare the capability without an authorizer", async () => {
      const { dir, binary, listen, replies } = await authorizingHost([]);
      try {
        const handle = await startRustHost({
          binary,
          home: dir,
          env: { FAKE_HOST_CAPTURE: listen, FAKE_HOST_REPLIES: replies },
        });
        await handle.stop();
        expect(JSON.parse(await readFile(listen, "utf8"))).not.toHaveProperty("localMediaPreview");
      } finally {
        await rm(dir, { recursive: true, force: true });
      }
    });
  },
);
