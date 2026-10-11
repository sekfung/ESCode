import assert from "node:assert/strict";
import test from "node:test";
import { dirname, join, resolve } from "node:path";
import { mkdtemp, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { fixture, event, end, type Harness } from "./escode-cli-rust-fixture.js";
import { configureRegistry } from "./escode-cli-rust-registry-fixture.js";

// docs/specs/rust-dynamic-workflow.md M3：V4 工作流只读查询。一条发布 markdown 产物、读过工作区文件的 run 结算后，
// 比对 workflowRuns / workflowRunEvents / workflowRunArtifacts / workflowRunArtifactRead / workflowRunWorkspace /
// workflowRunNodeResult 的应答（id / 时间戳抹掉）。
process.env.ESCODE_TEST_WAIT_MS ??= "60000";
const nodeBundle = resolve("apps/escode-cli/packages/cli/dist/escode.cjs");
const script = [
  'const paths = await files.glob("*.txt");',
  'await artifact.markdown("report", `# Found ${paths.length}`, { title: "Count report" });',
  "return { count: paths.length };",
].join("\n");

const call = (id: string, name: string, args: unknown) => ({
  tool_calls: [{ index: 0, id, type: "function", function: { name, arguments: JSON.stringify(args) } }],
});

async function observe(kind: "node" | "rust") {
  const root = await mkdtemp(join(tmpdir(), `escode-workflow-queries-${kind}-`));
  const requests: any[] = [];
  const respond = (req: any, res: any) => {
    requests.push(req);
    res.writeHead(200, { "content-type": "text/event-stream" });
    const n = requests.length;
    if (n === 1) event(res, call("skill", "Skill", { skill: "dynamic-workflows" }));
    else if (n === 2) event(res, call("cw-1", "CreateWorkflow", { script, name: "count-txt" }));
    else event(res, { content: n === 3 ? "waiting" : "done" });
    end(res, n <= 2 ? "tool_calls" : "stop");
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
    const h: Harness = f.start();
    const workspace = { workspacePath: h.workspace, workspaceKey: h.workspace };
    const parse = { parse: (value: unknown) => value } as any;
    await h.client.request("workspace/updateDynamicWorkflowPolicy", { workspace, enabled: true }, parse);
    const sessionId = await h.create();
    await h.subscribe(`conversation/${sessionId}`);
    let seen = 0;
    const answered = new Set<string>();
    const pump = setInterval(() => {
      for (; seen < h.messages.length; seen++) {
        for (const delta of h.messages[seen]?.params?.frame?.payload?.deltas ?? []) {
          for (const p of delta.patch?.pendingInteractions ?? []) {
            if (p.kind !== "permission" || answered.has(p.interactionId)) continue;
            answered.add(p.interactionId);
            const once = p.payload.options.find((o: any) => /allow.?once/i.test(o.kind));
            void h.command(
              h.envelope("resolveInteraction", sessionId, {
                interactionId: p.interactionId,
                answer: { optionId: once.optionId },
              }),
            );
          }
        }
      }
    }, 20);
    await h.command(h.envelope("sendText", sessionId, { text: "count", mode: "yolo" }));
    const deadline = Date.now() + 60_000;
    while (requests.length < 4 && Date.now() < deadline) {
      await new Promise((done) => setTimeout(done, 100));
    }
    await h.completed(sessionId);
    clearInterval(pump);
    const q = async (method: string, params: Record<string, unknown>) => {
      try {
        return (await h.client.request(`v4/conversation/${method}` as any, { sessionId, ...params }, parse)) as any;
      } catch (error) {
        return { error: error instanceof Error ? error.message : String(error) };
      }
    };
    const runs = await q("workflowRuns", {});
    const runId = runs.runs?.[0]?.runId;
    const events = await q("workflowRunEvents", { runId, limit: 3 });
    const artifacts = await q("workflowRunArtifacts", { runId });
    const first = artifacts.artifacts?.[0];
    const read = await q("workflowRunArtifactRead", {
      runId,
      artifactId: first?.id,
      version: first?.version ?? 1,
      offset: 0,
      limit: 1024,
    });
    const workspaceNodes = await q("workflowRunWorkspace", { runId });
    const node = workspaceNodes.nodes?.[0];
    const nodeResult = node ? await q("workflowRunNodeResult", { runId, siteId: node.siteId, ordinal: node.ordinal }) : null;
    await h.close();
    const rootText = JSON.stringify(root).slice(1, -1);
    const scrub = (value: unknown) =>
      JSON.parse(
        JSON.stringify(value)
          .replaceAll(rootText, "<root>")
          .replace(/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/g, "<uuid>")
          .replace(/"(\w*(?:At|Ms|ms|Time|time))":\d+(\.\d+)?/g, '"$1":0')
          .replace(/\d+(\.\d+)?\s?(ms|s)\b/g, "<duration>"),
      );
    return {
      ...scrub({ runs, events, artifacts, read, workspaceNodes, nodeResult }),
      schemaErrors: h.schemaErrors,
    };
  } finally {
    await f.close();
  }
}

test("Node and Rust answer the V4 workflow run queries the same way", async () => {
  const node = await observe("node");
  const rust = await observe("rust");
  if (process.env.DEBUG_WQ) console.log(JSON.stringify({ node, rust }, null, 1).slice(0, 8000));
  assert.deepEqual(node.schemaErrors, []);
  assert.equal(node.runs.runs.length, 1);
  assert.equal(node.artifacts.artifacts.length, 1);
  assert.equal(Buffer.from(node.read.dataBase64, "base64").toString(), "# Found 1");
  assert.deepEqual(node.nodeResult.result, ["a.txt"]);
  for (const key of Object.keys(node)) {
    assert.deepEqual((rust as any)[key], (node as any)[key], key);
  }
});
