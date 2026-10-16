import { describe, expect, it } from "vitest";
import { existsSync } from "node:fs";
import { copyFile, mkdir, mkdtemp, realpath, rm, writeFile } from "node:fs/promises";
import { createServer } from "node:http";
import { execFileSync } from "node:child_process";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { connectRpc, hostBinary, nextMeaningful, startHost } from "./rustHostTestSupport.js";

// docs/specs/rust-u2-git.md §7：git.generateCommitMessage 全链路——Host 读 git 状态/差分 → ModelSelection 视图的首选模型 →
// 内嵌 runtime 的 workspace/generateText → 本地假模型服务。prompt 与校验的逐字一致由黄金样本守住。

const BUILTIN_SOURCE = resolve("config/provider/escode-builtin.json");

const manualConfig = {
  enabled: true,
  properties: {
    contextWindow: 64000,
    supportsJsonSchemaOutput: false,
    supportsNativeWebSearch: false,
    supportsMidConversationSystem: false,
    inputFormat: { supportsImage: false, supportsVideo: false, supportsPdf: false },
  },
  optionSpecs: {
    reasoningLevel: { values: ["fast"], map: "{}" },
    maxOutputTokens: { max: 4096 },
  },
};

function git(cwd: string, ...args: string[]) {
  execFileSync("git", args, {
    cwd,
    stdio: "ignore",
    env: {
      ...process.env,
      GIT_AUTHOR_NAME: "t",
      GIT_AUTHOR_EMAIL: "t@t",
      GIT_COMMITTER_NAME: "t",
      GIT_COMMITTER_EMAIL: "t@t",
    },
  });
}

describe("rust host git.generateCommitMessage", () => {
  it.skipIf(!existsSync(hostBinary) || !existsSync(BUILTIN_SOURCE))(
    "turns the workspace changes into a validated commit message through the runtime",
    async () => {
      const base = await realpath(await mkdtemp(join(tmpdir(), "escode-rust-host-commitmsg-")));
      const prompts: string[] = [];
      let reply = "feat: describe the change";
      let failing = false;
      const server = createServer((request, response) => {
        const chunks: Buffer[] = [];
        request.on("data", (c) => chunks.push(c));
        request.on("end", () => {
          const body = JSON.parse(Buffer.concat(chunks).toString("utf8") || "{}");
          prompts.push(JSON.stringify(body.messages ?? []));
          if (failing) {
            response.writeHead(401, { "content-type": "application/json" });
            response.end(JSON.stringify({ error: { message: "mock upstream exploded" } }));
            return;
          }
          if (body.stream) {
            response.writeHead(200, { "content-type": "text/event-stream" });
            const frame = (delta: unknown, finish: string | null = null) =>
              `data: ${JSON.stringify({ id: "c", object: "chat.completion.chunk", model: "mock-model", choices: [{ index: 0, delta, finish_reason: finish }] })}\n\n`;
            response.write(frame({ role: "assistant", content: reply }));
            response.write(frame({}, "stop"));
            response.end("data: [DONE]\n\n");
          } else {
            response.writeHead(200, { "content-type": "application/json" });
            response.end(
              JSON.stringify({
                id: "c",
                object: "chat.completion",
                model: "mock-model",
                choices: [
                  {
                    index: 0,
                    message: { role: "assistant", content: reply },
                    finish_reason: "stop",
                  },
                ],
                usage: { prompt_tokens: 1, completion_tokens: 1, total_tokens: 2 },
              }),
            );
          }
        });
      });
      const port = await new Promise<number>((r) =>
        server.listen(0, "127.0.0.1", () => r((server.address() as { port: number }).port)),
      );
      const home = join(base, "home");
      const workspace = join(base, "repo");
      await mkdir(workspace, { recursive: true });
      git(workspace, "init", "-q");
      await writeFile(join(workspace, "a.txt"), "one\n");
      git(workspace, "add", ".");
      git(workspace, "commit", "-q", "-m", "init");
      await writeFile(join(workspace, "a.txt"), "one\ntwo\n");

      const builtin = join(base, "escode-builtin.json");
      const personal = join(base, "provider_config.json");
      await copyFile(BUILTIN_SOURCE, builtin);
      await writeFile(
        personal,
        JSON.stringify({
          schemaVersion: 1,
          config: {
            providerConfigRules: {
              providerRules: [
                {
                  providerId: "p-mock",
                  providerName: "Mock",
                  config: {
                    group: "standard-personal",
                    access: { type: "api-key", apiKey: "sk-mock" },
                    api: {
                      type: "openai-chat-completions",
                      baseUrl: `http://127.0.0.1:${port}/v1`,
                    },
                    personalModelIds: ["mock-model"],
                  },
                },
              ],
            },
            modelConfigRules: {
              providerModelRules: [],
              manualProviderModelRules: [
                { providerId: "p-mock", modelId: "mock-model", config: manualConfig },
              ],
            },
          },
        }),
      );
      const host = startHost(home);
      try {
        expect((await nextMeaningful(host)).kind).toBe("hostReady");
        host.send({
          kind: "rpcListen",
          bind: "127.0.0.1:0",
          home,
          dataBaseDir: base,
          runtimeEnv: {
            ESCODE_BUILTIN_PROVIDER_CONFIG_FILE: builtin,
            ESCODE_PERSONAL_PROVIDER_CONFIG_FILE: personal,
          },
        });
        const listening = await nextMeaningful(host);
        const services = await connectRpc(String(listening.addr), String(listening.secret));
        const gitService = services.gitService;

        const ok = await gitService.generateCommitMessage({
          workspacePath: workspace,
          locale: "en-US",
        } as never);
        expect(ok).toEqual({
          message: "feat: describe the change",
          providerId: "p-mock",
          model: "mock-model",
        });
        expect(prompts.length).toBeGreaterThan(0);
        expect(prompts.at(-1)).toContain("Changed files:");
        expect(prompts.at(-1)).toContain("a.txt");
        expect(prompts.at(-1)).toContain("Current language: English");

        reply = "```\nfix(core): wrapped in a fence\n```";
        await expect(
          gitService.generateCommitMessage({ workspacePath: workspace } as never),
        ).resolves.toMatchObject({ message: "fix(core): wrapped in a fence" });

        reply = "Sure! Here is a message for you";
        await expect(
          gitService.generateCommitMessage({ workspacePath: workspace } as never),
        ).rejects.toThrow("模型没有返回可用的 Conventional Commit 提交消息。");

        await expect(
          gitService.generateCommitMessage({
            workspacePath: workspace,
            currentSessionFilePaths: ["unrelated.txt"],
          } as never),
        ).rejects.toThrow("There are no changes available to commit.");

        // provider-settings.testModelConnectivity 的成功/失败路径（P3 之后一直缺端点，未验证）。
        const settings = services.providerSettingsService;
        const target = { workspacePath: workspace, providerId: "p-mock", modelId: "mock-model" };
        await expect(settings.testModelConnectivity(target as never)).resolves.toEqual({
          success: true,
        });
        failing = true;
        const failed = (await settings.testModelConnectivity(target as never)) as {
          success: boolean;
          error?: { message: string };
        };
        expect(failed.success).toBe(false);
        expect(failed.error?.message).toBeTruthy();
      } finally {
        await host.stop();
        server.close();
        await rm(base, { recursive: true, force: true });
      }
    },
    180_000,
  );
});
