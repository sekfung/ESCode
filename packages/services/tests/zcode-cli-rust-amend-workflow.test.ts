import assert from "node:assert/strict";
import test from "node:test";
import { dirname, join, resolve } from "node:path";
import { mkdtemp, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { fixture, event, end, type Harness } from "./zcode-cli-rust-fixture.js";
import { configureRegistry } from "./zcode-cli-rust-registry-fixture.js";

// docs/specs/rust-dynamic-workflow.md M3：AmendWorkflow。CreateWorkflow 结算后，用修订脚本修订同一个 run：
// 修订本会话的 run 免确认（workflowOwner 规则）→ 新 run 后台启动 → 结算 → 完成通知续跑。两侧模型面一致。
process.env.ZCODE_TEST_WAIT_MS ??= "60000";
type Message = Record<string, any>;
const nodeBundle = resolve("apps/zcode-cli/packages/cli/dist/zcode.cjs");
const script = ['const paths = await files.glob("*.txt");', "return { count: paths.length };"].join("\n");
const revised = [
  'const paths = await files.glob("*.txt");',
  'log("revised");',
  "return { count: paths.length, revised: true };",
].join("\n");

const call = (id: string, name: string, args: unknown) => ({
  tool_calls: [
    { index: 0, id, type: "function", function: { name, arguments: JSON.stringify(args) } },
  ],
});
const toolResult = (req: any, id: string) =>
  String(req?.messages?.find((m: any) => m.role === "tool" && m.tool_call_id === id)?.content ?? "");

async function observe(kind: "node" | "rust") {
  const root = await mkdtemp(join(tmpdir(), `zcode-amend-workflow-${kind}-`));
  const requests: any[] = [];
  const respond = (req: any, res: any) => {
    requests.push(req);
    res.writeHead(200, { "content-type": "text/event-stream" });
    const n = requests.length;
    if (n === 1) event(res, call("skill", "Skill", { skill: "dynamic-workflows" }));
    else if (n === 2) event(res, call("cw-1", "CreateWorkflow", { script, name: "count-txt" }));
    else if (n === 4) {
      const runId = /dwfrun-[A-Za-z0-9_-]+/.exec(toolResult(requests[2], "cw-1"))?.[0] ?? "missing";
      event(res, call("aw-1", "AmendWorkflow", { run_id: runId, script: revised }));
    } else event(res, { content: n === 3 || n === 5 ? "waiting" : "done" });
    end(res, n === 1 || n === 2 || n === 4 ? "tool_calls" : "stop");
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
    const permissions: any[] = [];
    let seen = 0;
    const answered = new Set<string>();
    const pump = setInterval(() => {
      for (; seen < h.messages.length; seen++) {
        for (const delta of h.messages[seen]?.params?.frame?.payload?.deltas ?? []) {
          for (const p of delta.patch?.pendingInteractions ?? []) {
            if (p.kind !== "permission" || answered.has(p.interactionId)) continue;
            answered.add(p.interactionId);
            permissions.push({
              toolName: p.payload.toolName,
              kinds: p.payload.options.map((o: Message) => o.kind),
            });
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
    const deadline = Date.now() + 90_000;
    while (requests.length < 6 && Date.now() < deadline) {
      await new Promise((done) => setTimeout(done, 100));
    }
    await h.completed(id);
    clearInterval(pump);
    await h.close();
    const rootText = JSON.stringify(root).slice(1, -1);
    const scrub = (text: string) =>
      text
        .replaceAll(rootText, "<root>")
        .replaceAll(root, "<root>")
        .replace(/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/g, "<uuid>")
        .replace(/dwf[-_][A-Za-z0-9_-]+/g, "<runId>")
        .replace(/\d+(\.\d+)?\s?(ms|s)\b/g, "<duration>");
    const lastUser = (req: any) => {
      const content = req?.messages?.filter((m: any) => m.role === "user").at(-1)?.content;
      return scrub(typeof content === "string" ? content : JSON.stringify(content));
    };
    return {
      amendResult: scrub(toolResult(requests[4], "aw-1")),
      notification: lastUser(requests[5]),
      permissions,
      requestCount: requests.length,
      schemaErrors: h.schemaErrors,
    };
  } finally {
    await f.close();
  }
}

test("Node and Rust amend a settled workflow run the same way", async () => {
  const node = await observe("node");
  const rust = await observe("rust");
  assert.deepEqual(node.schemaErrors, []);
  assert.match(node.amendResult, /background/i);
  assert.match(node.notification, /&quot;revised&quot;: true/);
  // 修订本会话自己的 run 免确认（workflowOwner 规则）：只有 CreateWorkflow 弹过一次。
  assert.equal(node.permissions.length, 1);
  for (const key of Object.keys(node)) {
    assert.deepEqual((rust as any)[key], (node as any)[key], key);
  }
});
