import assert from "node:assert/strict";
import test from "node:test";
import { dirname, join, resolve } from "node:path";
import { mkdtemp, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { fixture, event, end, type Harness } from "./zcode-cli-rust-fixture.js";
import { configureRegistry } from "./zcode-cli-rust-registry-fixture.js";

// docs/specs/rust-dynamic-workflow.md M3：TaskOutput 读工作流 run。阻塞读取拿到终态产物并认领完成通知（之后不再
// 开后台结果轮）；未知的 run id 是可修复的工具失败。宿主复用 TS 的后台追踪器与运行时任务注册表。
process.env.ZCODE_TEST_WAIT_MS ??= "60000";
type Message = Record<string, any>;
const nodeBundle = resolve("apps/zcode-cli/packages/cli/dist/zcode.cjs");
const script = ['const paths = await files.glob("*.txt");', "return { count: paths.length };"].join("\n");

const call = (id: string, name: string, args: unknown) => ({
  tool_calls: [
    { index: 0, id, type: "function", function: { name, arguments: JSON.stringify(args) } },
  ],
});
const toolResult = (req: any, id: string) =>
  String(req?.messages?.find((m: any) => m.role === "tool" && m.tool_call_id === id)?.content ?? "");

async function observe(kind: "node" | "rust") {
  const root = await mkdtemp(join(tmpdir(), `zcode-workflow-task-output-${kind}-`));
  const requests: any[] = [];
  const respond = (req: any, res: any) => {
    requests.push(req);
    res.writeHead(200, { "content-type": "text/event-stream" });
    const n = requests.length;
    if (n === 1) event(res, call("skill", "Skill", { skill: "dynamic-workflows" }));
    else if (n === 2) event(res, call("cw-1", "CreateWorkflow", { script, name: "count-txt" }));
    else if (n === 3) {
      const runId = /dwfrun-[A-Za-z0-9_-]+/.exec(toolResult(req, "cw-1"))?.[0] ?? "missing";
      event(res, call("to-1", "TaskOutput", { task_id: runId, block: true, timeout: 30000 }));
    } else if (n === 4) {
      event(res, call("to-2", "TaskOutput", { task_id: "dwfrun-unknown", block: false, timeout: 0 }));
    } else event(res, { content: "done" });
    end(res, n <= 4 ? "tool_calls" : "stop");
  };
  const env = {
    ZCODE_OFFICIAL_PLUGINS_BASE_DIR: dirname(nodeBundle),
    ZCODE_PLUGIN_HOST_EXEC_PATH: process.execPath,
    ZCODE_PLUGIN_HOST_ENTRYPOINT: nodeBundle,
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
          env,
        })
      : await fixture({ root, registry: true, respond, mode: "yolo", env });
  try {
    await configureRegistry(f, false);
    await writeFile(join(f.cwd, "a.txt"), "a");
    const h: Harness = f.start();
    const workspace = { workspacePath: h.workspace, workspaceKey: h.workspace };
    await h.client.request("workspace/updateDynamicWorkflowPolicy", { workspace, enabled: true }, {
      parse: (value: unknown) => value,
    } as any);
    const id = await h.create();
    await h.subscribe(`conversation/${id}`);
    let seen = 0;
    const answered = new Set<string>();
    const pump = setInterval(() => {
      for (; seen < h.messages.length; seen++) {
        for (const delta of h.messages[seen]?.params?.frame?.payload?.deltas ?? []) {
          for (const p of delta.patch?.pendingInteractions ?? []) {
            if (p.kind !== "permission" || answered.has(p.interactionId)) continue;
            answered.add(p.interactionId);
            const once = p.payload.options.find((o: Message) => /allow.?once/i.test(o.kind));
            void h.command(
              h.envelope("resolveInteraction", id, {
                interactionId: p.interactionId,
                answer: { optionId: once.optionId },
              }),
            );
          }
        }
      }
    }, 20);
    await h.command(h.envelope("sendText", id, { text: "count the txt files", mode: "yolo" }));
    const deadline = Date.now() + 60_000;
    while (requests.length < 5 && Date.now() < deadline) {
      await new Promise((done) => setTimeout(done, 100));
    }
    await h.completed(id);
    // 完成通知已被 TaskOutput 认领：再等一会儿确认没有后台结果轮。
    await new Promise((done) => setTimeout(done, 1500));
    clearInterval(pump);
    await h.close();
    const scrub = (text: string) =>
      text
        .replace(/dwf[a-z]*[-_][A-Za-z0-9_-]+/g, "<id>")
        .replace(/\d+(\.\d+)?\s?(ms|s)\b/g, "<duration>");
    return {
      blockingRead: scrub(toolResult(requests[3], "to-1")),
      unknownRun: scrub(toolResult(requests[4], "to-2")),
      requestCount: requests.length,
      schemaErrors: h.schemaErrors,
    };
  } finally {
    await f.close();
  }
}

test("Node and Rust read a workflow run with TaskOutput the same way", async () => {
  const node = await observe("node");
  const rust = await observe("rust");
  assert.deepEqual(node.schemaErrors, []);
  assert.match(node.blockingRead, /<status>completed<\/status>/);
  assert.equal(node.requestCount, 5);
  for (const key of Object.keys(node)) {
    assert.deepEqual((rust as any)[key], (node as any)[key], key);
  }
});
