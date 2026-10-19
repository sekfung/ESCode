//! `grantWorkspaceHookTrust` 的真实 Agent authority 端到端：真实 `@escode/client` → Rust Host →
//! Rust runtime → 真实 `escode.cjs __escode-workflow-host` → trust store。
//!
//! 这是 hooks 域唯一不经过桩的授权链路；`escode.cjs` 未构建时显式跳过（CI 的
//! `test:escode-cli-rust` 会先构建它；`rustHostHooksParity.test.ts` 另有转发契约用例）。
import { describe, expect, it } from "vitest";
import { existsSync } from "node:fs";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { connectRpc, hostBinary, nextMeaningful, startHost } from "./rustHostTestSupport.js";

const escodeCjs = resolve("apps/escode-cli/packages/cli/dist/escode.cjs");

describe.skipIf(!existsSync(hostBinary) || !existsSync(escodeCjs))(
  "rust host hooks trust authority (requires escode.cjs)",
  () => {
    it("grants an exact declaration through the real authority and rejects a stale bundle", async () => {
      const root = await mkdtemp(join(tmpdir(), "escode-rust-hooks-authority-"));
      const home = join(root, "home");
      const workspace = join(root, "workspace");
      const dataBaseDir = join(root, "data");
      for (const path of [home, workspace, dataBaseDir]) await mkdir(path, { recursive: true });
      await mkdir(join(workspace, ".git"), { recursive: true });
      await mkdir(join(workspace, ".escode"), { recursive: true });
      await writeFile(
        join(workspace, ".escode", "config.json"),
        JSON.stringify({
          hooks: {
            enabled: true,
            events: {
              PreToolUse: [
                {
                  matcher: "Bash",
                  hooks: [{ type: "process", command: "node", args: ["hook.mjs"] }],
                },
              ],
            },
          },
        }),
      );

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
            ESCODE_PLUGIN_HOST_ENTRYPOINT: escodeCjs,
          },
        });
        const listening = await nextMeaningful(host);
        const services = await connectRpc(String(listening.addr), String(listening.secret));
        const loaded = (await services.hooksService.loadHooks({
          workspacePath: workspace,
          workspaceIdentity: workspace,
        } as never)) as {
          workspaceHookSnapshot: {
            bundleDigest: string;
            hooks: Array<{ hookDeclarationDigest: string }>;
          };
        };
        const snapshot = loaded.workspaceHookSnapshot;
        const digest = snapshot.hooks[0].hookDeclarationDigest;

        await expect(
          services.hooksService.grantWorkspaceHookTrust?.({
            workspacePath: workspace,
            workspaceIdentity: workspace,
            bundleDigest: snapshot.bundleDigest,
            hookDeclarationDigest: digest,
          }),
        ).resolves.toEqual({ accepted: true });

        const store = JSON.parse(
          await readFile(join(home, ".escode", "security", "workspace-hook-trust-v1.json"), "utf8"),
        ) as { records: Array<Record<string, unknown>> };
        expect(store.records).toEqual([
          expect.objectContaining({
            workspaceIdentity: workspace,
            hookDeclarationDigest: digest,
            decision: "trusted",
            eventAtGrant: "PreToolUse",
          }),
        ]);

        // 过期 bundle：authority 重新发现后拒绝，Host 原样透传 reasonCode。
        await expect(
          services.hooksService.grantWorkspaceHookTrust?.({
            workspacePath: workspace,
            workspaceIdentity: workspace,
            bundleDigest: "0".repeat(64),
            hookDeclarationDigest: digest,
          }),
        ).resolves.toEqual({
          accepted: false,
          reasonCode: "workspace_hooks_bundle_changed",
        });
      } finally {
        await host.stop();
        await rm(root, { recursive: true, force: true });
      }
    }, 300_000);
  },
);
