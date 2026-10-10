import assert from "node:assert/strict";
import test from "node:test";
import { join, resolve } from "node:path";
import { mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { fixture, event, end } from "./escode-cli-rust-fixture.js";
import { configureRegistry } from "./escode-cli-rust-registry-fixture.js";

// docs/specs/rust-dynamic-workflow.md 第 6 期：灰度为开时 ListModels 的模型面、行级 display 与
// 目录事实（当前档、档位表、上下文窗）在 Node 与 Rust 上一致。
process.env.ESCODE_TEST_WAIT_MS ??= "30000";
const nodeBundle = resolve("apps/escode-cli/packages/cli/dist/escode.cjs");
const text = (m: any) => (typeof m.content === "string" ? m.content : JSON.stringify(m.content));

async function observe(kind: "node" | "rust") {
  const root = await mkdtemp(join(tmpdir(), `escode-list-models-${kind}-`));
  const requests: any[] = [];
  const respond = (req: any, res: any) => {
    requests.push(req);
    res.writeHead(200, { "content-type": "text/event-stream" });
    const answered = req.messages.some((m: any) => m.role === "tool");
    if (!answered) {
      event(res, {
        tool_calls: [
          {
            index: 0,
            id: "call-models",
            type: "function",
            function: { name: "ListModels", arguments: "{}" },
          },
        ],
      });
      end(res, "tool_calls");
      return;
    }
    event(res, { content: "done" });
    end(res, "stop");
  };
  const f =
    kind === "node"
      ? await fixture({
          root,
          command: process.execPath,
          args: ({ cwd }) => [nodeBundle, "app-server", "--stdio", "--cwd", cwd],
          registry: true,
          respond,
          mode: "yolo",
        })
      : await fixture({ root, registry: true, respond, mode: "yolo" });
  try {
    // 两个模型都带档位表；model-b 额外带上下文窗，用来验证目录事实的在场规则。
    await configureRegistry(f, false, {
      properties: { contextWindow: 131072 },
    });
    const h = f.start();
    await h.client.request(
      "workspace/updateDynamicWorkflowPolicy",
      { workspace: { workspacePath: h.workspace, workspaceKey: h.workspace }, enabled: true },
      { parse: (value: unknown) => value } as any,
    );
    const id = await h.create();
    await h.subscribe(`conversation/${id}`);
    await h.command(h.envelope("sendText", id, { text: "list models", mode: "yolo" }));
    await h.completed(id);
    const rows = (await h.rows(id)) as { rows: any[] };
    await h.close();
    const toolMessage = requests
      .flatMap((request) => request.messages ?? [])
      .filter((message) => message.role === "tool")
      .map((message) => text(message))
      .at(-1);
    const row = rows.rows.find((candidate) => candidate.toolName === "ListModels");
    return { tool: toolMessage, display: row?.display ?? null, status: row?.status };
  } finally {
    await f.close();
  }
}

test("ListModels matches Node for a configured registry", async () => {
  const node = await observe("node");
  const rust = await observe("rust");
  // 自检：两行目录、档位表在场、会话模型标 current、上下文窗在场。
  assert.match(node.tool ?? "", /^<models count="2">/);
  assert.match(
    node.tool ?? "",
    /personal:fixture\/model-a — Fixture; levels: low,high \(default high\) \[current\]/,
  );
  assert.match(
    node.tool ?? "",
    /personal:fixture\/model-b — Fixture; levels: low,high \(default high\)/,
  );
  assert.equal(node.display?.kind, "list_models");
  assert.equal(node.display?.current, "personal:fixture/model-a");
  assert.deepEqual(
    node.display?.models?.map((model: any) => model.contextWindow),
    [131072, 131072],
  );
  // 两侧逐字/逐值一致。
  assert.deepEqual(rust.tool, node.tool);
  assert.deepEqual(rust.display, node.display);
  assert.deepEqual(rust.status, node.status);
});
