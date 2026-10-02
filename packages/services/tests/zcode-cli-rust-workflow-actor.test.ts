import assert from "node:assert/strict";
import test from "node:test";
import { dirname, join, resolve } from "node:path";
import { mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { fixture, event, end, type Harness } from "./zcode-cli-rust-fixture.js";
import { configureRegistry } from "./zcode-cli-rust-registry-fixture.js";

// docs/specs/rust-dynamic-workflow.md「M2 设计」：带 actor 的 CreateWorkflow。actor 的每一轮在 Rust 子会话执行，
// submit_result 回宿主由 TS driver 裁决。比对 actor 的模型请求（系统提示词、工具面、消息）、CreateWorkflow
// 结果与完成通知（run id / 路径 / 耗时抹掉）。
process.env.ZCODE_TEST_WAIT_MS ??= "60000";
type Message = Record<string, any>;
const nodeBundle = resolve("apps/zcode-cli/packages/cli/dist/zcode.cjs");
const script = [
  "interface Answer { total: number; }",
  'const counter = agent("counter", { system: "You count things carefully." });',
  'phase("Count the items");',
  'const answer = await counter.ask<Answer>("How many items are there? Answer with a total.");',
  "return { total: answer.total };",
].join("\n");

const call = (id: string, name: string, args: unknown) => ({
  tool_calls: [
    { index: 0, id, type: "function", function: { name, arguments: JSON.stringify(args) } },
  ],
});

async function observe(kind: "node" | "rust") {
  const root = await mkdtemp(join(tmpdir(), `zcode-workflow-actor-${kind}-`));
  const main: any[] = [];
  const actor: any[] = [];
  const respond = (req: any, res: any) => {
    res.writeHead(200, { "content-type": "text/event-stream" });
    const isActor = (req.tools ?? []).some((t: any) => t.function?.name === "submit_result");
    if (isActor) {
      actor.push(req);
      if (actor.length === 1) {
        event(res, call("submit-1", "submit_result", { result: { total: 3 } }));
        end(res, "tool_calls");
      } else {
        event(res, { content: "submitted" });
        end(res, "stop");
      }
      return;
    }
    main.push(req);
    const steps = [
      call("skill", "Skill", { skill: "dynamic-workflows" }),
      call("cw-1", "CreateWorkflow", { script, name: "count-items" }),
    ];
    const step = steps[main.length - 1];
    if (step) {
      event(res, step);
      end(res, "tool_calls");
    } else {
      event(res, { content: main.length === steps.length + 1 ? "waiting" : "done" });
      end(res, "stop");
    }
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
    await h.command(h.envelope("sendText", id, { text: "count the items", mode: "yolo" }));
    const deadline = Date.now() + 60_000;
    while (main.length < 4 && Date.now() < deadline) {
      await new Promise((done) => setTimeout(done, 100));
    }
    await h.completed(id);
    clearInterval(pump);
    await h.close();
    const rootText = JSON.stringify(root).slice(1, -1);
    const scrub = (value: unknown) =>
      JSON.parse(
        JSON.stringify(value)
          .replaceAll(rootText, "<root>")
          .replace(/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/g, "<uuid>")
          .replace(/\d+(\.\d+)?\s?(ms|s)\b/g, "<duration>")
          .replace(/Today's date is [^.]*\./g, "Today's date is <date>."),
      );
    const first = actor[0];
    const notification = main[3]?.messages?.filter((m: any) => m.role === "user").at(-1)?.content;
    return scrub({
      actorSystem: (first?.messages ?? []).filter((m: any) => m.role === "system").map((m: any) => m.content),
      actorUser: (first?.messages ?? []).filter((m: any) => m.role === "user").map((m: any) => m.content),
      actorTools: (first?.tools ?? []).map((t: any) => t.function.name).sort(),
      submitTool: (first?.tools ?? []).find((t: any) => t.function.name === "submit_result"),
      actorRequests: actor.length,
      createResult: main[2]?.messages?.find((m: any) => m.tool_call_id === "cw-1")?.content,
      notification,
      schemaErrors: h.schemaErrors,
    });
  } finally {
    await f.close();
  }
}

test("Node and Rust run a workflow actor ask the same way", async () => {
  const node = await observe("node");
  const rust = await observe("rust");
  assert.deepEqual(node.schemaErrors, []);
  assert.ok(node.actorRequests >= 1, "the actor ran at least one model request");
  assert.match(String(node.notification), /&quot;total&quot;: 3/);
  for (const key of Object.keys(node)) {
    assert.deepEqual((rust as any)[key], (node as any)[key], key);
  }
});
