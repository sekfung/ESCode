import assert from "node:assert/strict";
import test from "node:test";
import { join, resolve } from "node:path";
import { mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { DatabaseSync } from "node:sqlite";
import { fixture, event, end } from "./escode-cli-rust-fixture.js";
import { configureRegistry } from "./escode-cli-rust-registry-fixture.js";

// docs/specs/rust-dynamic-workflow.md 第 6 期：ListWorkflowRuns 在 Node 与 Rust 上一致。
//
// 注意两侧的库是**两个文件**：Node 用 `ts.sqlite`，Rust 用自己的 `data/rust-sessions.sqlite`
// （`ESCODE_SESSION_DB_PATH` 在 Rust 侧只用于导入）。所以先各起一次把 dwf_* 表迁移出来，
// 再往两个库里播**同一组** run 行，然后比较两侧读出的模型面与 display。
// 「读得懂 TS 写的库」由存储层语料（同一份 DDL/行）另行覆盖。
process.env.ESCODE_TEST_WAIT_MS ??= "30000";
const nodeBundle = resolve("apps/escode-cli/packages/cli/dist/escode.cjs");
const text = (m: any) => (typeof m.content === "string" ? m.content : JSON.stringify(m.content));
const NOW = 1_700_000_000_000;

function seed(dbPath: string) {
  const db = new DatabaseSync(dbPath);
  const insert = (table: string, row: Record<string, unknown>) => {
    const columns = Object.keys(row);
    db.prepare(
      `insert into ${table} (${columns.join(", ")}) values (${columns.map(() => "?").join(", ")})`,
    ).run(...columns.map((column) => row[column] as any));
  };
  const run = (id: string, overrides: Record<string, unknown> = {}) => ({
    id,
    parent_session_id: "sess_other",
    cwd: "",
    name: null,
    script_text: "return 1;",
    script_hash: "hash",
    args_json: null,
    tool_call_id: null,
    resumed_from: null,
    caps_max_concurrency: 4,
    spent_tokens: 7,
    status: "completed",
    result_json: null,
    failure_json: null,
    time_created: NOW,
    time_updated: NOW,
    ...overrides,
  });
  return { db, insert, run };
}

async function observe(kind: "node" | "rust", root: string) {
  const requests: any[] = [];
  const respond = (req: any, res: any) => {
    res.writeHead(200, { "content-type": "text/event-stream" });
    const answers = req.messages.filter((m: any) => m.role === "tool").length;
    // 第一次调用不带过滤，第二次按状态过滤（SQL 谓词的下推面）。
    if (answers < 2) {
      event(res, {
        tool_calls: [
          {
            index: 0,
            id: `call-runs-${answers}`,
            type: "function",
            function: {
              name: "ListWorkflowRuns",
              arguments: answers === 0 ? "{}" : JSON.stringify({ statuses: ["stopped"] }),
            },
          },
        ],
      });
      end(res, "tool_calls");
      requests.push(req);
      return;
    }
    requests.push(req);
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
    await h.client.request(
      "workspace/updateDynamicWorkflowPolicy",
      { workspace: { workspacePath: h.workspace, workspaceKey: h.workspace }, enabled: true },
      { parse: (value: unknown) => value } as any,
    );
    const id = await h.create();
    await h.subscribe(`conversation/${id}`);
    await h.command(h.envelope("sendText", id, { text: "list runs", mode: "yolo" }));
    await h.completed(id);
    const rows = (await h.rows(id)) as { rows: any[] };
    await h.close();
    // 取最后一次请求里的工具结果：它带着两次调用各自的答复（早先请求里的历史是同一批）。
    const messages = (requests.at(-1)?.messages ?? [])
      .filter((message: any) => message.role === "tool")
      .map((message: any) => text(message));
    const displays = rows.rows
      .filter((candidate) => candidate.toolName === "ListWorkflowRuns")
      .map((candidate) => candidate.display);
    return { messages, displays };
  } finally {
    await f.close();
  }
}

/** 起一次 runtime 并立刻关闭：让它的存储完成迁移（dwf_* 表就位），供随后播种子行。 */
async function migrate(kind: "node" | "rust", root: string) {
  const f = await fixture(
    kind === "node"
      ? {
          root,
          command: process.execPath,
          args: ({ cwd }) => [nodeBundle, "app-server", "--stdio", "--cwd", cwd],
          registry: true,
          mode: "yolo",
        }
      : { root, registry: true, mode: "yolo" },
  );
  await configureRegistry(f, false);
  f.start();
  await f.close();
}

test("ListWorkflowRuns matches Node for a seeded journal", async () => {
  const root = await mkdtemp(join(tmpdir(), "escode-list-workflow-runs-"));
  await migrate("node", root);
  await migrate("rust", root);
  for (const dbPath of [join(root, "ts.sqlite"), join(root, "data", "rust-sessions.sqlite")]) {
    const { db, insert, run } = seed(dbPath);
    insert("dwf_run", run("run-completed", { cwd: join(root, "workspace"), name: "review" }));
    insert(
      "dwf_run",
      run("run-stopped", {
        cwd: join(root, "workspace"),
        status: "cancelled",
        failure_json: '{"stopReason":"user"}',
        time_updated: NOW + 10,
      }),
    );
    insert(
      "dwf_run",
      run("run-running-sibling", {
        cwd: join(root, "workspace"),
        status: "running",
        failure_json: null,
        time_updated: NOW + 20,
      }),
    );
    db.close();
  }

  const node = await observe("node", root);
  const rust = await observe("rust", root);
  // 自检：三次运行（completed / stopped / running），两条消息分别是全量与被过滤的 stopped。
  assert.match(node.messages[0] ?? "", /^<workflow_runs count="3">/);
  assert.match(node.messages[0] ?? "", /id="run-stopped" status="stopped" stop_reason="user"/);
  assert.match(node.messages[0] ?? "", /id="run-running-sibling" status="running"/);
  assert.match(node.messages[0] ?? "", /possibly_interrupted="true"/);
  assert.match(node.messages[1] ?? "", /^<workflow_runs count="1">/);
  assert.match(node.messages[1] ?? "", /id="run-stopped"/);
  assert.deepEqual(rust.messages, node.messages);
  assert.deepEqual(rust.displays, node.displays);
});
