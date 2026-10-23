//! hooks 域差分：冻结的 TS `createHooksService()` 观察作 oracle，经真实 `@escode/client` 调 Rust Host。
//!
//! Phase 3 golden freeze：测试不再实例化 TS 服务，改为回放
//! `fixtures/golden/rustHostHooksParity.oracle.json` 中冻结的配置与观察；
//! 删除 TS 实现后仍能独立重验 Rust Host。
//!
//! 覆盖（docs/specs/rust-u2-hooks.md §9）：
//! - L4：同一批 TS 写出的配置，Rust `loadHooks` 与冻结的 TS `loadHooks` 结构全等（`discoveredAt` 除外）；
//! - L5：同一 Hook[] 由 Rust `saveHooks` 写回，与冻结的 TS 落盘文件逐字节一致；
//! - 存量读入：TS 写出的 config / trust store 被 Rust 读入；
//! - 真实客户端错误路径与 grant 转发（最小 workflow host 桩）。
import { describe, expect, it } from "vitest";
import { existsSync } from "node:fs";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { connectRpc, hostBinary, nextMeaningful, startHost } from "./rustHostTestSupport.js";

const originalHome = process.env.HOME;
const originalUserProfile = process.env.USERPROFILE;

// 冻结的 TS 观察：绝对临时根统一写成 `$FIXTURE_ROOT`，运行时替换为本次临时根。
const goldenFile = new URL("./fixtures/golden/rustHostHooksParity.oracle.json", import.meta.url);

interface Golden {
  expected: {
    loaded: Record<string, unknown> & {
      workspaceHookSnapshot: {
        bundleDigest: string;
        hooks: Array<{ hookDeclarationDigest: string }>;
      };
    };
    trusted: Record<string, unknown>;
    saved: { userConfig: string; projectConfig: string };
  };
}

function normalize(result: Record<string, unknown>): Record<string, unknown> {
  // 线上语义是 JSON：TS 对象里的 `undefined` 属性不会出现在 RPC 应答中；
  // 先过一遍 JSON 去掉它们，再替换自然生成的时间戳（与生成脚本一致）。
  const wire = JSON.parse(JSON.stringify(result)) as Record<string, unknown>;
  const snapshot = wire.workspaceHookSnapshot as Record<string, unknown> | undefined;
  return {
    ...wire,
    workspaceHookSnapshot: snapshot ? { ...snapshot, discoveredAt: "<time>" } : undefined,
  };
}

const fixtureHooks = [
  {
    id: "user-hook",
    event: "SessionStart",
    matcher: "startup",
    type: "process",
    command: "node",
    args: ["user.mjs"],
    enabled: true,
    custom: { flag: true },
    location: { source: "escode", scope: "user" },
  },
  {
    id: "project-hook",
    event: "PreToolUse",
    matcher: "Bash",
    type: "command",
    command: "./hook.sh",
    async: true,
    shell: "bash",
    statusMessage: "Preparing",
    timeout: 12,
    enabled: true,
    location: { source: "escode", scope: "project" },
  },
  {
    id: "skip-legacy",
    event: "Stop",
    type: "command",
    command: "echo skip",
    enabled: true,
    location: { source: "agents", scope: "user" },
  },
];

describe.skipIf(!existsSync(hostBinary))("rust host hooks parity", () => {
  it("matches the frozen TypeScript observations and reads/writes the same files", async () => {
    const root = await mkdtemp(join(tmpdir(), "escode-rust-hooks-"));
    const home = join(root, "home");
    const workspace = join(root, "workspace");
    const dataBaseDir = join(root, "data");
    const userConfigPath = join(home, ".escode", "cli", "config.json");
    const projectConfigPath = join(workspace, ".escode", "config.json");
    for (const path of [home, workspace, dataBaseDir]) await mkdir(path, { recursive: true });
    const stub = join(root, "workflow-host.cjs");
    await writeFile(
      stub,
      `
const readline = require("node:readline");
const rl = readline.createInterface({ input: process.stdin });
rl.on("close", () => process.exit(0));
rl.on("line", (line) => {
  const message = JSON.parse(line);
  if (message.method === "init") return;
  const params = message.params && message.params.params ? message.params.params : {};
  const result = params.hookDeclarationDigest === "reject"
    ? { accepted: false, reasonCode: "workspace_hooks_bundle_changed" }
    : { accepted: true };
  process.stdout.write(JSON.stringify({ id: message.id, result }) + "\\n");
});
`,
    );
    process.env.HOME = home;
    process.env.USERPROFILE = home;
    const golden = JSON.parse(
      (await readFile(goldenFile, "utf8")).split("$FIXTURE_ROOT").join(root),
    ) as Golden;

    // 回放 TS 写出的存量文件（冻结的 L5 基线；未知键 features 必须保留）。
    await mkdir(join(home, ".escode", "cli"), { recursive: true });
    await mkdir(join(workspace, ".escode"), { recursive: true });
    await writeFile(userConfigPath, golden.expected.saved.userConfig);
    await writeFile(projectConfigPath, golden.expected.saved.projectConfig);

    const host = startHost(home);
    try {
      await nextMeaningful(host);
      host.send({
        kind: "rpcListen",
        bind: "127.0.0.1:0",
        home,
        dataBaseDir,
        runtimeEnv: {
          ESCODE_PLUGIN_HOST_EXEC_PATH: process.execPath,
          ESCODE_PLUGIN_HOST_ENTRYPOINT: stub,
        },
      });
      const listening = await nextMeaningful(host);
      const services = await connectRpc(String(listening.addr), String(listening.secret));

      // L4 + 存量读入：TS 写、Rust 读，除 discoveredAt 外与冻结 TS 观察全等。
      const rustLoaded = (await services.hooksService.loadHooks({
        workspacePath: workspace,
        workspaceIdentity: workspace,
      } as never)) as Record<string, unknown>;
      expect(normalize(rustLoaded)).toEqual(golden.expected.loaded);
      expect(rustLoaded.workspaceHookSnapshot).toBeDefined();

      // L5：重置后 Rust 写一次，与冻结的 TS 落盘文件逐字节比较。
      await rm(userConfigPath);
      await rm(join(workspace, ".escode"), { recursive: true, force: true });
      await mkdir(join(home, ".escode", "cli"), { recursive: true });
      await writeFile(userConfigPath, JSON.stringify({ features: { mcp: true } }));
      await services.hooksService.saveHooks({
        workspacePath: workspace,
        workspaceIdentity: workspace,
        hooks: fixtureHooks,
      } as never);
      const rustUser = await readFile(userConfigPath, "utf8");
      const rustProject = await readFile(projectConfigPath, "utf8");
      expect(rustUser).toBe(golden.expected.saved.userConfig);
      expect(rustProject).toBe(golden.expected.saved.projectConfig);

      // 恢复 TS 写出的配置，继续存量读入场景。
      await rm(userConfigPath);
      await rm(join(workspace, ".escode"), { recursive: true, force: true });
      await mkdir(join(home, ".escode", "cli"), { recursive: true });
      await mkdir(join(workspace, ".escode"), { recursive: true });
      await writeFile(userConfigPath, golden.expected.saved.userConfig);
      await writeFile(projectConfigPath, golden.expected.saved.projectConfig);

      // trust store 存量读入：TS 形状的持久信任命中。
      const snapshot = golden.expected.loaded.workspaceHookSnapshot;
      const digest = snapshot.hooks[0].hookDeclarationDigest;
      await mkdir(join(home, ".escode", "security"), { recursive: true });
      await writeFile(
        join(home, ".escode", "security", "workspace-hook-trust-v1.json"),
        JSON.stringify({
          schemaVersion: 1,
          records: [
            {
              workspaceIdentity: workspace,
              hookDeclarationDigest: digest,
              digestAlgorithm: "sha256",
              decision: "trusted",
              grantedAt: new Date().toISOString(),
              eventAtGrant: "PreToolUse",
              displayCommandAtGrant: "./hook.sh",
              sourcePathAtGrant: ".escode/config.json",
            },
          ],
        }),
      );
      const rustTrusted = (await services.hooksService.loadHooks({
        workspacePath: workspace,
        workspaceIdentity: workspace,
      } as never)) as {
        hooks: Array<{ workspaceHook?: { trustState?: string } }>;
      };
      expect(normalize(rustTrusted as unknown as Record<string, unknown>)).toEqual(
        golden.expected.trusted,
      );
      expect(rustTrusted.hooks[0]?.workspaceHook?.trustState).toBe("trusted_persistent");

      // 真实客户端 grant：Host 转发运行时，桩返回成功/拒绝。
      await expect(
        services.hooksService.grantWorkspaceHookTrust?.({
          workspacePath: workspace,
          workspaceIdentity: workspace,
          bundleDigest: snapshot.bundleDigest,
          hookDeclarationDigest: digest,
        }),
      ).resolves.toEqual({ accepted: true });
      await expect(
        services.hooksService.grantWorkspaceHookTrust?.({
          workspacePath: workspace,
          workspaceIdentity: workspace,
          bundleDigest: snapshot.bundleDigest,
          hookDeclarationDigest: "reject",
        }),
      ).resolves.toEqual({
        accepted: false,
        reasonCode: "workspace_hooks_bundle_changed",
      });
      await expect(
        services.hooksService.grantWorkspaceHookTrust?.({
          workspacePath: workspace,
        } as never),
      ).rejects.toThrow(/requires bundleDigest/);

      // 相对路径显式拒绝（已登记差异）。
      await expect(
        services.hooksService.loadHooks({ workspacePath: "relative/workspace" } as never),
      ).rejects.toThrow(/must be absolute/);
    } finally {
      process.env.HOME = originalHome;
      if (originalUserProfile === undefined) delete process.env.USERPROFILE;
      else process.env.USERPROFILE = originalUserProfile;
      await host.stop();
      await rm(root, { recursive: true, force: true });
    }
  }, 300_000);
});
