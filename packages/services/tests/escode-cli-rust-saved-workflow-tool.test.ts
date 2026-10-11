import assert from "node:assert/strict";
import test from "node:test";
import { join, resolve } from "node:path";
import { mkdir, mkdtemp, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { fixture, event, end } from "./escode-cli-rust-fixture.js";
import { configureRegistry } from "./escode-cli-rust-registry-fixture.js";

// docs/specs/rust-dynamic-workflow.md 第 2 期：ListSavedWorkflows 在灰度为开时，Node 与 Rust
// 对同一份项目档给出的模型面内容与行级 display 一致（路径按 workspace 归一后逐字比较）。
// 灰度关闭时该工具两侧都不注册，由 dynamic-workflow-policy 用例覆盖。
process.env.ESCODE_TEST_WAIT_MS ??= "30000";
const nodeBundle = resolve("apps/escode-cli/packages/cli/dist/escode.cjs");
const text = (m: any) => (typeof m.content === "string" ? m.content : JSON.stringify(m.content));
const WORKFLOW_FILE = [
  "/* escode-workflow",
  "description: Review the pull request end to end.",
  "whenToUse: When a pull request is open",
  "args:",
  "  pr:",
  "    type: string",
  "    required: true",
  "    description: PR number or URL",
  "  depth:",
  "    type: number",
  "    default: 3",
  "*/",
  "export default async () => {",
  "  return 1;",
  "}",
  "",
].join("\n");

async function observe(kind: "node" | "rust") {
  const root = await mkdtemp(join(tmpdir(), `escode-saved-workflow-tool-${kind}-`));
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
            id: "call-list",
            type: "function",
            function: { name: "ListSavedWorkflows", arguments: "{}" },
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
    await configureRegistry(f, false);
    const h = f.start();
    const scoped = join(h.workspace, ".escode", "workflows");
    await mkdir(scoped, { recursive: true });
    await writeFile(join(scoped, "review.dwf.ts"), WORKFLOW_FILE);
    await h.client.request(
      "workspace/updateDynamicWorkflowPolicy",
      { workspace: { workspacePath: h.workspace, workspaceKey: h.workspace }, enabled: true },
      { parse: (value: unknown) => value } as any,
    );
    const id = await h.create();
    await h.subscribe(`conversation/${id}`);
    await h.command(h.envelope("sendText", id, { text: "list workflows", mode: "yolo" }));
    await h.completed(id);
    const rows = (await h.rows(id)) as { rows: any[] };
    await h.close();
    const normalize = (value: string) =>
      value.replaceAll(h.workspace, "<workspace>").replaceAll(h.workspace.replaceAll("\\", "/"), "<workspace>");
    // JSON 文本里的路径反斜杠是转义的，先取 JSON.stringify 后的形态再替换。
    const jsonWorkspace = JSON.stringify(h.workspace).slice(1, -1);
    const toolMessage = requests
      .flatMap((request) => request.messages ?? [])
      .filter((message) => message.role === "tool")
      .map((message) => normalize(text(message)))
      .at(-1);
    const row = rows.rows.find((candidate) => candidate.toolName === "ListSavedWorkflows");
    return {
      tool: toolMessage,
      display:
        row?.display === undefined
          ? null
          : JSON.parse(JSON.stringify(row.display).replaceAll(jsonWorkspace, "<workspace>")),
      status: row?.status,
    };
  } finally {
    await f.close();
  }
}

test("ListSavedWorkflows matches Node for the project archive", async () => {
  const node = await observe("node");
  const rust = await observe("rust");
  // 自检：模型面确实是清单，而不是空容器或别的东西。
  assert.match(node.tool ?? "", /^<saved_workflows count="1">/);
  assert.match(node.tool ?? "", /<workflow name="review" scope="project">/);
  assert.match(node.tool ?? "", /arg pr \(string, required\) — PR number or URL/);
  assert.equal(node.display?.kind, "saved_workflow_list");
  assert.deepEqual(rust.tool, node.tool);
  assert.deepEqual(rust.display, node.display);
  assert.deepEqual(rust.status, node.status);
});
