import assert from "node:assert/strict";
import test from "node:test";
import { dirname, resolve } from "node:path";
import { fixture, event, end, type Harness } from "./zcode-cli-rust-fixture.js";
import { configureRegistry } from "./zcode-cli-rust-registry-fixture.js";

// 内置 `/workflow`（TS builtin-workflow-command.ts）：动态工作流开启的会话展开成带技能前言的提示词，关闭的会话
// 原文作普通提示词。userInput 行仍显示原文。比对两种会话第一次模型请求的末条用户消息与行文本。
process.env.ZCODE_TEST_WAIT_MS ??= "60000";
const nodeBundle = resolve("apps/zcode-cli/packages/cli/dist/zcode.cjs");
const input = "/workflow count the txt files";

async function observe(kind: "node" | "rust") {
  const requests: any[] = [];
  const respond = (req: any, res: any) => {
    requests.push(req);
    res.writeHead(200, { "content-type": "text/event-stream" });
    event(res, { content: "ok" });
    end(res, "stop");
  };
  const env = {
    ZCODE_OFFICIAL_PLUGINS_BASE_DIR: dirname(nodeBundle),
    ZCODE_PLUGIN_HOST_EXEC_PATH: process.execPath,
    ZCODE_PLUGIN_HOST_ENTRYPOINT: nodeBundle,
  };
  const f =
    kind === "node"
      ? await fixture({
          command: process.execPath,
          args: ({ cwd }) => [nodeBundle, "app-server", "--stdio", "--cwd", cwd],
          registry: true,
          respond,
          mode: "yolo",
          env,
        })
      : await fixture({ registry: true, respond, mode: "yolo", env });
  try {
    await configureRegistry(f, false);
    const h: Harness = f.start();
    const run = async (id: string) => {
      await h.subscribe(`conversation/${id}`);
      const before = requests.length;
      await h.command(h.envelope("sendText", id, { text: input, mode: "yolo" }));
      await h.completed(id);
      const req = requests[before];
      const rows = (await h.rows(id)).rows;
      return {
        model: req?.messages?.filter((m: any) => m.role === "user").at(-1)?.content,
        row: rows.find((r: any) => r.kind === "userInput")?.text,
      };
    };
    const workspace = { workspacePath: h.workspace, workspaceKey: h.workspace };
    const parse = { parse: (value: unknown) => value } as any;
    // App `/` 目录：内置 `workflow` 受进程级动态工作流开关约束（TS zcode-protocol/slash-commands.ts）。
    const catalog = async () =>
      ((await h.client.request("workspace/readPresentation", { workspace }, parse)) as any).slashCommands
        .filter((c: any) => c.source === "builtin")
        .map((c: any) => c.name);
    const catalogBefore = await catalog();
    const disabled = await run(await h.create());
    await h.client.request("workspace/updateDynamicWorkflowPolicy", { workspace, enabled: true }, parse);
    const catalogAfter = await catalog();
    const enabled = await run(await h.create());
    await h.close();
    return { catalogBefore, catalogAfter, disabled, enabled, schemaErrors: h.schemaErrors };
  } finally {
    await f.close();
  }
}

test("Node and Rust expand the builtin /workflow command the same way", async () => {
  const node = await observe("node");
  const rust = await observe("rust");
  assert.deepEqual(node.schemaErrors, []);
  assert.match(String(node.enabled.model), /Run custom command \/workflow\./);
  assert.equal(node.disabled.model, input);
  for (const key of Object.keys(node)) {
    assert.deepEqual((rust as any)[key], (node as any)[key], key);
  }
});
