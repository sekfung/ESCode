import assert from "node:assert/strict";
import test from "node:test";
import { dirname, join, resolve } from "node:path";
import { mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { fixture, event, end, type Harness } from "./escode-cli-rust-fixture.js";
import { configureRegistry } from "./escode-cli-rust-registry-fixture.js";

// docs/specs/rust-v4-command-gaps.md：run 卡 / 详情页的「取消」（cancelBackgroundWork {workId: dwfrun-…}）与
// 「恢复」（resumeWorkflowRun）走用户命令面。actor 首个请求挂住 → UI 取消 → UI 恢复 → actor 重跑并结算。
// 比对两侧的命令 ACK、停止 / 完成通知与拒绝路径（未知 run、已完结 run）。
process.env.ESCODE_TEST_WAIT_MS ??= "60000";
type Message = Record<string, any>;
const nodeBundle = resolve("apps/escode-cli/packages/cli/dist/escode.cjs");
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
  String(
    req?.messages?.find((m: any) => m.role === "tool" && m.tool_call_id === id)?.content ?? "",
  );
const lastUser = (req: any) => {
  const content = req?.messages?.filter((m: any) => m.role === "user").at(-1)?.content;
  return typeof content === "string" ? content : String(JSON.stringify(content));
};
const until = async (condition: () => boolean, ms = 60_000) => {
  const deadline = Date.now() + ms;
  while (!condition() && Date.now() < deadline) await new Promise((done) => setTimeout(done, 50));
  return condition();
};

async function observe(kind: "node" | "rust") {
  const root = await mkdtemp(join(tmpdir(), `escode-workflow-run-commands-${kind}-`));
  const main: any[] = [];
  const actor: any[] = [];
  let runId = "missing";
  let hung: any;
  const respond = (req: any, res: any) => {
    res.writeHead(200, { "content-type": "text/event-stream" });
    if ((req.tools ?? []).some((t: any) => t.function?.name === "submit_result")) {
      actor.push(req);
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
    } else {
      if (n === 3) runId = /dwfrun-[A-Za-z0-9_-]+/.exec(toolResult(req, "cw-1"))?.[0] ?? "missing";
      event(res, { content: "noted" });
      end(res, "stop");
    }
  };
  const env = {
    ESCODE_OFFICIAL_PLUGINS_BASE_DIR: dirname(nodeBundle),
    ESCODE_PLUGIN_HOST_EXEC_PATH: process.execPath,
    ESCODE_PLUGIN_HOST_ENTRYPOINT: nodeBundle,
  };
  let pump: ReturnType<typeof setInterval> | undefined;
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
    pump = setInterval(() => {
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
    const ack = async (type: string, payload: Message) => {
      const result: any = await h.command(h.envelope(type, id, payload));
      return {
        status: result.status,
        reasonCode: result.reasonCode ?? null,
        message:
          typeof result.message === "string"
            ? result.message.replace(/dwfrun-[A-Za-z0-9_-]+/g, "<run>")
            : null,
      };
    };
    await h.command(h.envelope("sendText", id, { text: "count the items", mode: "yolo" }));
    await h.completed(id);
    assert.ok(await until(() => actor.length >= 1), `${kind}: actor never started`);

    const cancel = await ack("cancelBackgroundWork", { workId: runId });
    try {
      event(hung, { content: "late" });
      end(hung, "stop");
    } catch {
      // 已被客户端断开。
    }
    const stopped = await until(() => main.length >= 4, 30_000);
    const resume = await ack("resumeWorkflowRun", { workId: runId, name: "count-items" });
    const settled = await until(() => main.length >= 5 && actor.length >= 3, 60_000);
    // 拒绝路径：未知 run、已完结 run 的取消与恢复。
    const resumeMissing = await ack("resumeWorkflowRun", { workId: "dwfrun-missing" });
    const cancelMissing = await ack("cancelBackgroundWork", { workId: "dwfrun-missing" });
    const cancelDone = await ack("cancelBackgroundWork", { workId: runId });
    clearInterval(pump);
    await h.close();
    const scrub = (text: string) =>
      text
        .replaceAll(JSON.stringify(root).slice(1, -1), "<root>")
        .replaceAll(root, "<root>")
        .replace(/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/g, "<uuid>")
        .replace(/dwf[a-z]*[-_][A-Za-z0-9_-]+/g, "<id>")
        .replace(/\d+(\.\d+)?\s?(ms|s)\b/g, "<duration>")
        .replace(/Today's date is [^.]*\./g, "Today's date is <date>.");
    return {
      cancel,
      stopped,
      stopNotice: scrub(lastUser(main[3])),
      resume,
      settled,
      finalNotice: scrub(lastUser(main[4])),
      resumeMissing,
      cancelMissing,
      cancelDone,
      actorRequests: actor.length,
      schemaErrors: h.schemaErrors,
    };
  } finally {
    clearInterval(pump);
    await f.close();
  }
}

test("Node and Rust cancel and resume a workflow run from the UI command surface the same way", async () => {
  const node = await observe("node");
  const rust = await observe("rust");
  assert.deepEqual(node.schemaErrors, []);
  assert.equal(node.cancel.status, "accepted");
  assert.equal(node.resume.status, "accepted");
  for (const key of Object.keys(node)) {
    assert.deepEqual((rust as any)[key], (node as any)[key], key);
  }
});
