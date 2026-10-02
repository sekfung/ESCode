import assert from "node:assert/strict";
import test from "node:test";
import { dirname, join, resolve } from "node:path";
import { mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { fixture, event, end, type Harness } from "./zcode-cli-rust-fixture.js";
import { configureRegistry } from "./zcode-cli-rust-registry-fixture.js";

// docs/specs/rust-dynamic-workflow.md M3：升级问答。actor 调 `escalate` 停驻 → 父会话收到 run 中通知（后台结果轮）
// → 主代理用 ResolveWorkflowQuestion 作答 → actor 的 escalate 结果就是答案，接着 submit_result → run 结算通知。
// 比对升级通知、作答结果、actor 收到的 escalate 结果与终态通知（run id / qid / 耗时抹掉）。
process.env.ZCODE_TEST_WAIT_MS ??= "60000";
type Message = Record<string, any>;
const nodeBundle = resolve("apps/zcode-cli/packages/cli/dist/zcode.cjs");
const script = [
  "interface Answer { dir: string; }",
  'const scout = agent("scout", { system: "You locate things." });',
  'const answer = await scout.ask<Answer>("Which directory holds the sources?");',
  "return { dir: answer.dir };",
].join("\n");

const call = (id: string, name: string, args: unknown) => ({
  tool_calls: [
    { index: 0, id, type: "function", function: { name, arguments: JSON.stringify(args) } },
  ],
});
const toolResult = (req: any, id: string) =>
  String(req?.messages?.find((m: any) => m.role === "tool" && m.tool_call_id === id)?.content ?? "");
const lastUser = (req: any) => {
  const content = req?.messages?.filter((m: any) => m.role === "user").at(-1)?.content;
  return typeof content === "string" ? content : JSON.stringify(content);
};

async function observe(kind: "node" | "rust") {
  const root = await mkdtemp(join(tmpdir(), `zcode-workflow-escalation-${kind}-`));
  const main: any[] = [];
  const actor: any[] = [];
  const respond = (req: any, res: any) => {
    res.writeHead(200, { "content-type": "text/event-stream" });
    if ((req.tools ?? []).some((t: any) => t.function?.name === "submit_result")) {
      actor.push(req);
      if (actor.length === 1) {
        event(res, call("esc-1", "escalate", { question: "Which directory should I report?" }));
        end(res, "tool_calls");
      } else if (actor.length === 2) {
        event(res, call("submit-1", "submit_result", { result: { dir: "src" } }));
        end(res, "tool_calls");
      } else {
        event(res, { content: "submitted" });
        end(res, "stop");
      }
      return;
    }
    main.push(req);
    const n = main.length;
    if (n === 1) event(res, call("skill", "Skill", { skill: "dynamic-workflows" }));
    else if (n === 2) event(res, call("cw-1", "CreateWorkflow", { script, name: "find-sources" }));
    else if (n === 4) {
      const qid = /dwfq-[A-Za-z0-9_-]+/.exec(lastUser(req))?.[0] ?? "missing";
      event(res, call("rq-1", "ResolveWorkflowQuestion", { question_id: qid, answer: "Report src." }));
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
    await h.command(h.envelope("sendText", id, { text: "find the sources", mode: "yolo" }));
    const deadline = Date.now() + 90_000;
    while (main.length < 6 && Date.now() < deadline) {
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
        .replace(/dwf[a-z]*[-_][A-Za-z0-9_-]+/g, "<id>")
        .replace(/\d+(\.\d+)?\s?(ms|s)\b/g, "<duration>");
    return {
      escalationNotice: scrub(lastUser(main[3])),
      resolveResult: scrub(toolResult(main[4], "rq-1")),
      actorEscalateResult: scrub(toolResult(actor[1], "esc-1")),
      finalNotice: scrub(lastUser(main[5])),
      mainRequests: main.length,
      actorRequests: actor.length,
      schemaErrors: h.schemaErrors,
    };
  } finally {
    await f.close();
  }
}

test("Node and Rust route a workflow escalation through the parent the same way", async () => {
  const node = await observe("node");
  const rust = await observe("rust");
  assert.deepEqual(node.schemaErrors, []);
  assert.match(node.escalationNotice, /Which directory should I report\?/);
  assert.match(node.actorEscalateResult, /Report src\./);
  assert.match(node.finalNotice, /&quot;dir&quot;: &quot;src&quot;/);
  for (const key of Object.keys(node)) {
    assert.deepEqual((rust as any)[key], (node as any)[key], key);
  }
});
