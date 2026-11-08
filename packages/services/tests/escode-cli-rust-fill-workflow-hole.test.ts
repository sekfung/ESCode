import assert from "node:assert/strict";
import test from "node:test";
import { dirname, join, resolve } from "node:path";
import { mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { fixture, event, end, type Harness } from "./escode-cli-rust-fixture.js";
import { configureRegistry } from "./escode-cli-rust-registry-fixture.js";
import { canonical, goldenOracle } from "./escode-cli-rust-golden.js";

// docs/specs/rust-dynamic-workflow.md：FillWorkflowHole 的本会话 run 免确认。
// 同一段带留白的脚本在两侧跑到停驻，随后补全；只有 CreateWorkflow 弹一次确认，
// FillWorkflowHole 读宿主回填的 `hole.owned_by_this_session` 直接放行。
process.env.ESCODE_TEST_WAIT_MS ??= "60000";
type Message = Record<string, any>;
const pluginHostBundle = resolve("apps/escode-cli/packages/cli/dist/plugin-host.cjs");
const script = [
  'phase("准备");',
  'const v = await hole<string>("裁决", "decide");',
  "return v;",
].join("\n");
const fillBody = 'return "ok";';

const call = (id: string, name: string, args: unknown) => ({
  tool_calls: [
    { index: 0, id, type: "function", function: { name, arguments: JSON.stringify(args) } },
  ],
});
const toolResult = (req: any, id: string) =>
  String(
    req?.messages?.find((m: any) => m.role === "tool" && m.tool_call_id === id)?.content ?? "",
  );
const lastUser = (req: any) => {
  const content = req?.messages?.filter((m: any) => m.role === "user").at(-1)?.content;
  return typeof content === "string" ? content : JSON.stringify(content ?? "");
};

async function observe(kind: "node" | "rust") {
  const root = await mkdtemp(join(tmpdir(), `escode-fill-workflow-hole-${kind}-`));
  const requests: any[] = [];
  const respond = (req: any, res: any) => {
    requests.push(req);
    res.writeHead(200, { "content-type": "text/event-stream" });
    const n = requests.length;
    if (n === 1) event(res, call("skill", "Skill", { skill: "dynamic-workflows" }));
    else if (n === 2) event(res, call("cw-1", "CreateWorkflow", { script, name: "hole-run" }));
    else if (n === 4) {
      const notice = lastUser(req);
      const runId = /run_id="([^"]+)"/.exec(notice)?.[1] ?? "missing";
      const holeId = /hole_id="([^"]+)"/.exec(notice)?.[1] ?? "missing";
      event(
        res,
        call("fh-1", "FillWorkflowHole", { run_id: runId, hole_id: holeId, script: fillBody }),
      );
    } else event(res, { content: n === 3 || n === 5 ? "waiting" : "done" });
    end(res, n === 1 || n === 2 || n === 4 ? "tool_calls" : "stop");
  };
  const env = {
    ESCODE_OFFICIAL_PLUGINS_BASE_DIR: dirname(pluginHostBundle),
    ESCODE_PLUGIN_HOST_EXEC_PATH: process.execPath,
    ESCODE_PLUGIN_HOST_ENTRYPOINT: pluginHostBundle,
  };
  const f =
    kind === "node"
      ? await fixture({
          root,
          command: process.execPath,
          args: ({ cwd }) => [pluginHostBundle, "app-server", "--stdio", "--cwd", cwd],
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
    await h.command(h.envelope("sendText", id, { text: "fill the hole", mode: "yolo" }));
    const deadline = Date.now() + 60_000;
    while (requests.length < 5 && Date.now() < deadline) {
      await new Promise((done) => setTimeout(done, 100));
    }
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
    return {
      createResult: scrub(toolResult(requests[2], "cw-1")),
      fillResult: scrub(toolResult(requests[4], "fh-1")),
      permissions,
      requestCount: requests.length,
      schemaErrors: h.schemaErrors,
    };
  } finally {
    await f.close();
  }
}

test("Node and Rust fill a hole in this session's run without asking again", async () => {
  const node = await goldenOracle("observe", () => observe("node"));
  const rust = canonical(await observe("rust"));
  assert.deepEqual(node.schemaErrors, []);
  assert.match(node.createResult, /background/i);
  assert.match(node.fillResult, /resumed/i);
  // 只有 CreateWorkflow 需要确认；本会话 run 的 FillWorkflowHole 不再弹窗。
  assert.deepEqual(
    node.permissions.map((permission: Message) => permission.toolName),
    ["CreateWorkflow"],
  );
  for (const key of Object.keys(node)) {
    assert.deepEqual((rust as any)[key], (node as any)[key], key);
  }
});
