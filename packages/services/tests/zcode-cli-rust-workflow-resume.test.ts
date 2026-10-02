import assert from "node:assert/strict";
import test from "node:test";
import { dirname, join, resolve } from "node:path";
import { mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { fixture, event, end, type Harness } from "./zcode-cli-rust-fixture.js";
import { configureRegistry } from "./zcode-cli-rust-registry-fixture.js";

// docs/specs/rust-dynamic-workflow.md M3：停止与恢复。actor 的第一次模型请求挂住时主代理 TaskStop 这条 run
// （stopped(model) 通知在步边界并入同一回合）→ ResumeWorkflowRun → actor 会话重水化（resumeFromStore）后重跑未完结的 ask →
// submit_result → 结算。比对 TaskStop / Resume 的结果、停止与终态通知，以及重水化后 actor 的请求消息。
process.env.ZCODE_TEST_WAIT_MS ??= "60000";
type Message = Record<string, any>;
const nodeBundle = resolve("apps/zcode-cli/packages/cli/dist/zcode.cjs");
const script = [
  "interface Answer { total: number; }",
  'const counter = agent("counter", { system: "You count things carefully." });',
  'const answer = await counter.ask<Answer>("How many items are there?");',
  "return { total: answer.total };",
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
  return typeof content === "string" ? content : String(JSON.stringify(content));
};

async function observe(kind: "node" | "rust") {
  const root = await mkdtemp(join(tmpdir(), `zcode-workflow-resume-${kind}-`));
  const main: any[] = [];
  const actor: any[] = [];
  let runId = "missing";
  let hung: any;
  const respond = (req: any, res: any) => {
    res.writeHead(200, { "content-type": "text/event-stream" });
    if ((req.tools ?? []).some((t: any) => t.function?.name === "submit_result")) {
      actor.push(req);
      // 第一次请求挂住，直到主代理拿到 TaskStop 的结果（停下的是一个未完结的 ask）。
      if (actor.length === 1) {
        hung = res;
        return;
      }
      if (actor.length === 2) {
        event(res, call("submit-1", "submit_result", { result: { total: 3 } }));
        end(res, "tool_calls");
      } else {
        event(res, { content: "submitted" });
        end(res, "stop");
      }
      return;
    }
    main.push(req);
    const n = main.length;
    if (n === 1) {
      event(res, call("skill", "Skill", { skill: "dynamic-workflows" }));
      end(res, "tool_calls");
    } else if (n === 2) {
      event(res, call("cw-1", "CreateWorkflow", { script, name: "count-items" }));
      end(res, "tool_calls");
    } else if (n === 3) {
      runId = /dwfrun-[A-Za-z0-9_-]+/.exec(toolResult(req, "cw-1"))?.[0] ?? "missing";
      // 等 actor 的请求在飞再停，保证停下的是一个未完结的 ask。
      const timer = setInterval(() => {
        if (actor.length === 0) return;
        clearInterval(timer);
        event(res, call("stop-1", "TaskStop", { task_id: runId }));
        end(res, "tool_calls");
      }, 20);
    } else if (n === 4) {
      try {
        event(hung, { content: "late" });
        end(hung, "stop");
      } catch {
        // 已被客户端断开。
      }
      // 停止通知在步边界并入本回合（task_notification_steer），模型随即恢复。
      event(res, call("rs-1", "ResumeWorkflowRun", { run_id: runId }));
      end(res, "tool_calls");
    } else {
      event(res, { content: n === 5 ? "waiting" : "done" });
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
        .replace(/\d+(\.\d+)?\s?(ms|s)\b/g, "<duration>")
        .replace(/Today's date is [^.]*\./g, "Today's date is <date>.");
    const resumed = (actor[1]?.messages ?? [])
      .filter((m: any) => m.role !== "system")
      .map((m: any) => ({
        role: m.role,
        content: scrub(typeof m.content === "string" ? m.content : JSON.stringify(m.content ?? null)),
        ...(m.tool_calls ? { tools: m.tool_calls.map((c: any) => c.function.name) } : {}),
      }));
    return {
      stopResult: scrub(toolResult(main[3], "stop-1")),
      stopNotice: scrub(lastUser(main[3])),
      resumeResult: scrub(toolResult(main[4], "rs-1")),
      resumedActorMessages: resumed,
      finalNotice: scrub(lastUser(main[5])),
      mainRequests: main.length,
      actorRequests: actor.length,
      schemaErrors: h.schemaErrors,
    };
  } finally {
    await f.close();
  }
}

test("Node and Rust stop and resume a workflow run with a live actor the same way", async () => {
  const node = await observe("node");
  const rust = await observe("rust");
  assert.deepEqual(node.schemaErrors, []);
  assert.match(node.stopNotice, /stopped/i);
  assert.match(node.finalNotice, /&quot;total&quot;: 3/);
  for (const key of Object.keys(node)) {
    assert.deepEqual((rust as any)[key], (node as any)[key], key);
  }
});
