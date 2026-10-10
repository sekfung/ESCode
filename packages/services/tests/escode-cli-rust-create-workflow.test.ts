import assert from "node:assert/strict";
import test from "node:test";
import { dirname, join, resolve } from "node:path";
import { mkdtemp, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { applyConversationDeltas } from "@escode/shared/escode-protocol-v4";
import { fixture, event, end, type Harness } from "./escode-cli-rust-fixture.js";
import { configureRegistry } from "./escode-cli-rust-registry-fixture.js";

// docs/specs/rust-dynamic-workflow.md「运行面实现计划」M1：CreateWorkflow（无 actor 脚本）。技能门之后提交 →
// alwaysAsk 确认（无「总是允许」）→ 后台启动 → run 结算 → 完成通知作为后台结果轮续跑。Rust 经工作流宿主
// 复用 TS run 服务与 handler；模型看到的工具结果与完成通知两侧一致（run id / 路径 / 耗时抹掉）。
process.env.ESCODE_TEST_WAIT_MS ??= "60000";
type Message = Record<string, any>;
const nodeBundle = resolve("apps/escode-cli/packages/cli/dist/escode.cjs");
const script = [
  'const paths = await files.glob("*.txt");',
  "log(`found ${paths.length}`);",
  "return { count: paths.length, paths };",
].join("\n");

const call = (id: string, name: string, args: unknown) => ({
  tool_calls: [
    { index: 0, id, type: "function", function: { name, arguments: JSON.stringify(args) } },
  ],
});
const steps = [
  call("skill", "Skill", { skill: "dynamic-workflows" }),
  call("cw-1", "CreateWorkflow", { script, name: "count-txt" }),
];

async function observe(kind: "node" | "rust") {
  const root = await mkdtemp(join(tmpdir(), `escode-create-workflow-${kind}-`));
  const requests: any[] = [];
  const respond = (req: any, res: any) => {
    requests.push(req);
    res.writeHead(200, { "content-type": "text/event-stream" });
    const step = steps[requests.length - 1];
    if (step) {
      event(res, step);
      end(res, "tool_calls");
    } else {
      event(res, { content: requests.length === steps.length + 1 ? "waiting" : "done" });
      end(res, "stop");
    }
  };
  const env = {
    ESCODE_OFFICIAL_PLUGINS_BASE_DIR: dirname(nodeBundle),
    ESCODE_PLUGIN_HOST_EXEC_PATH: process.execPath,
    ESCODE_PLUGIN_HOST_ENTRYPOINT: nodeBundle,
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
    await writeFile(join(f.cwd, "b.txt"), "b");
    const h: Harness = f.start();
    const workspace = { workspacePath: h.workspace, workspaceKey: h.workspace };
    await h.client.request("workspace/updateDynamicWorkflowPolicy", { workspace, enabled: true }, {
      parse: (value: unknown) => value,
    } as any);
    const id = await h.create();
    await h.subscribe(`conversation/${id}`);
    // 第二条订阅协商了 `workflowRunDeltas`：收 `workflowRun.*` 键级增量，state patch 不再带整键。
    const deltasSub = (await h.client.request(
      "v4/conversation/subscribe",
      { topic: `conversation/${id}`, connectionId: "fixture-deltas", clientMode: "desktop-continuous", workflowRunDeltas: true },
      { parse: (value: unknown) => value } as any,
    )) as any;
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
    // 等第二轮（后台完成通知）的模型请求与它的收尾。
    const deadline = Date.now() + 60_000;
    while (requests.length < steps.length + 2 && Date.now() < deadline) {
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
    const createResult = requests[steps.length]?.messages?.find(
      (m: any) => m.role === "tool" && m.tool_call_id === "cw-1",
    )?.content;
    // V4 `workflowRuns` 状态键的终值（未协商 workflowRunDeltas 的客户端收整键 state.updated）。
    let workflowRuns: unknown;
    for (const message of h.messages) {
      for (const delta of message?.params?.frame?.payload?.deltas ?? []) {
        if (delta.patch?.workflowRuns !== undefined) workflowRuns = delta.patch.workflowRuns;
      }
      const snapshot = message?.params?.frame?.payload?.snapshot;
      if (snapshot?.workflowRuns !== undefined) workflowRuns = snapshot.workflowRuns;
    }
    // 增量订阅：快照 + 依序应用全部增量帧后的 workflowRuns，以及 op 判别式序列。
    let applied: any;
    const opKinds: string[] = [];
    let patchCarriedRuns = false;
    for (const message of h.messages) {
      const frame = message?.params?.frame;
      if (!frame || frame.subscriptionId !== deltasSub.ack.subscriptionId) continue;
      if (frame.payload?.kind === "snapshot") applied = frame.payload.snapshot;
      for (const delta of frame.payload?.deltas ?? []) {
        if (String(delta.op).startsWith("workflowRun.")) opKinds.push(delta.op);
        if (delta.patch?.workflowRuns !== undefined) patchCarriedRuns = true;
      }
      if (frame.payload?.kind === "deltas" && applied) applied = applyConversationDeltas(applied, frame.payload.deltas);
    }
    const scrubState = (value: unknown) =>
      JSON.parse(
        scrub(JSON.stringify(value ?? null)).replace(/"(\w*(?:At|Ms|revision|Sequence))":\d+/g, '"$1":0'),
      );
    const notificationRequest = requests[steps.length + 1];
    const notification = notificationRequest?.messages
      ?.filter((m: any) => m.role === "user")
      .at(-1)?.content;
    return {
      createResult: scrub(String(createResult)),
      notification: scrub(typeof notification === "string" ? notification : JSON.stringify(notification)),
      permissions,
      requestCount: requests.length,
      workflowRuns: scrubState(workflowRuns),
      deltaWorkflowRuns: scrubState(applied?.workflowRuns),
      opKinds: [...new Set(opKinds)],
      patchCarriedRuns,
      schemaErrors: h.schemaErrors,
    };
  } finally {
    await f.close();
  }
}

test("Node and Rust run a CreateWorkflow script in the background the same way", async () => {
  const node = await observe("node");
  const rust = await observe("rust");
  if (process.env.DEBUG_PERM) console.log(JSON.stringify({ node: node.permissions, rust: rust.permissions }));
  assert.deepEqual(node.schemaErrors, []);
  // 自检：Node 侧确实后台启动、确认一次、结算后用完成通知续跑。
  assert.match(node.createResult, /background/i);
  assert.match(node.notification, /<task-notification>/);
  assert.match(node.notification, /&quot;count&quot;: 2/);
  assert.equal(node.permissions.length, 1);
  assert.equal((node.workflowRuns as any)?.runs?.[0]?.status, "completed");
  assert.deepEqual(node.deltaWorkflowRuns, node.workflowRuns);
  assert.deepEqual(node.opKinds, ["workflowRun.updated"]);
  assert.equal(node.patchCarriedRuns, false);
  for (const key of Object.keys(node)) {
    assert.deepEqual((rust as any)[key], (node as any)[key], key);
  }
});
