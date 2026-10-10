import assert from "node:assert/strict";
import test from "node:test";
import { dirname, join, resolve } from "node:path";
import { mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { fixture, event, end, type Harness } from "./escode-cli-rust-fixture.js";
import { configureRegistry } from "./escode-cli-rust-registry-fixture.js";

// 模型失败分类映射（docs/specs/rust-dynamic-workflow.md）：actor 的模型请求返回配额业务码 1308，TS driver 按
// inspectWorkflowModelFailure 把 run 停为 stopped(provider, quota)。比对 run 状态键、CreateWorkflow 结果与通知。
process.env.ESCODE_TEST_WAIT_MS ??= "60000";
const nodeBundle = resolve("apps/escode-cli/packages/cli/dist/escode.cjs");
const script = [
  "interface Answer { total: number; }",
  'const counter = agent("counter", { system: "You count things carefully." });',
  'const answer = await counter.ask<Answer>("How many items are there? Answer with a total.");',
  "return { answer };",
].join("\n");

const call = (id: string, name: string, args: unknown) => ({
  tool_calls: [{ index: 0, id, type: "function", function: { name, arguments: JSON.stringify(args) } }],
});

async function observe(kind: "node" | "rust") {
  const root = await mkdtemp(join(tmpdir(), `escode-workflow-failure-${kind}-`));
  const main: any[] = [];
  let actorRequests = 0;
  const respond = (req: any, res: any) => {
    const isActor = (req.tools ?? []).some((t: any) => t.function?.name === "submit_result");
    if (isActor) {
      actorRequests += 1;
      res.writeHead(429, { "content-type": "application/json" });
      res.end(JSON.stringify({ error: { code: "1308", message: "quota exhausted" } }));
      return;
    }
    main.push(req);
    res.writeHead(200, { "content-type": "text/event-stream" });
    const steps = [
      call("skill", "Skill", { skill: "dynamic-workflows" }),
      call("cw-1", "CreateWorkflow", { script, name: "count-items" }),
    ];
    const step = steps[main.length - 1];
    if (step) {
      event(res, step);
      end(res, "tool_calls");
    } else {
      event(res, { content: "noted" });
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
            const once = p.payload.options.find((o: any) => /allow.?once/i.test(o.kind));
            void h.command(h.envelope("resolveInteraction", id, { interactionId: p.interactionId, answer: { optionId: once.optionId } }));
          }
        }
      }
    }, 20);
    await h.command(h.envelope("sendText", id, { text: "count the items", mode: "yolo" }));
    const run = () => {
      let runs: any;
      for (const m of h.messages) {
        for (const delta of m?.params?.frame?.payload?.deltas ?? []) {
          if (delta.patch?.workflowRuns !== undefined) runs = delta.patch.workflowRuns;
        }
      }
      return runs?.runs?.[0];
    };
    const deadline = Date.now() + 60_000;
    while (!["stopped", "failed", "completed"].includes(run()?.status) && Date.now() < deadline) {
      await new Promise((done) => setTimeout(done, 100));
    }
    await new Promise((done) => setTimeout(done, 1500));
    await h.completed(id);
    clearInterval(pump);
    await h.close();
    const final = run();
    const scrub = (value: unknown) =>
      JSON.parse(
        JSON.stringify(value ?? null)
          .replace(/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/g, "<uuid>")
          .replace(/dwf[-_][A-Za-z0-9_-]+/g, "<runId>")
          .replace(/\d+(\.\d+)?\s?(ms|s)\b/g, "<duration>")
          .replace(/"(\w*(?:At|Ms|revision|Sequence))":\d+/g, '"$1":0'),
      );
    return {
      actorRetried: actorRequests > 1,
      status: final?.status,
      stop: scrub(final?.stop ?? final?.stopped ?? null),
      run: scrub(final),
      schemaErrors: h.schemaErrors,
    };
  } finally {
    await f.close();
  }
}

test("Node and Rust stop a workflow run on a provider quota failure the same way", async () => {
  const node = await observe("node");
  if (process.env.ESCODE_DUMP) console.log(JSON.stringify(node, null, 1));
  const rust = await observe("rust");
  assert.deepEqual(node.schemaErrors, []);
  assert.notEqual(node.status, "completed");
  for (const key of Object.keys(node)) {
    assert.deepEqual((rust as any)[key], (node as any)[key], key);
  }
});
