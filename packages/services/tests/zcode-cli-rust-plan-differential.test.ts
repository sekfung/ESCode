import assert from "node:assert/strict";
import test from "node:test";
import { join, resolve } from "node:path";
import { mkdtemp, readFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { fixture, event, end } from "./zcode-cli-rust-fixture.js";
import { configureRegistry } from "./zcode-cli-rust-registry-fixture.js";
import { z } from "zod";

// docs/specs/rust-plan-mode.md：plan 模式在 Node 与 Rust 上的协议投影与模型可见内容必须一致。
const nodeBundle = resolve("apps/zcode-cli/packages/cli/dist/zcode.cjs");
type Runtime = "node" | "rust";
type Answer = "approve" | "feedback" | "decline";
const PLAN = "1. Read the code\n2. Change it";

function toolCall(res: Parameters<typeof event>[0], name: string, args: unknown, id: string) {
  event(res, {
    tool_calls: [
      { index: 0, id, type: "function", function: { name, arguments: JSON.stringify(args) } },
    ],
  });
  end(res, "tool_calls");
}

const reminders = (messages: { role: string; content: unknown }[]) =>
  messages
    .map((m) => (typeof m.content === "string" ? m.content : JSON.stringify(m.content)))
    .filter((t) => t.includes("lan mode") || t.includes("Exited Plan Mode"))
    .map((t) => t.slice(0, 160));

export async function observePlan(kind: Runtime, answer: Answer) {
  const root = await mkdtemp(join(tmpdir(), `zcode-plan-${kind}-`));
  let step = 0;
  const respond = (req: any, res: any) => {
    res.writeHead(200, { "content-type": "text/event-stream" });
    const n = step++;
    if (n === 0) return toolCall(res, "ExitPlanMode", { plan: PLAN }, "exit-1");
    event(res, { content: "ok" });
    end(res, "stop");
  };
  const f =
    kind === "node"
      ? await fixture({
          root,
          command: process.execPath,
          args: ({ cwd }) => [nodeBundle, "app-server", "--stdio", "--cwd", cwd],
          registry: true,
          respond,
        })
      : await fixture({ root, registry: true, respond });
  try {
    await configureRegistry(f);
    const h = f.start();
    const capabilities = (await h.client.request("runtime/capabilities", {}, z.any())) as any;
    const id = await h.create();
    await h.subscribe(`conversation/${id}`);
    await h.command(
      h.envelope("sendText", id, { text: "plan it", mode: "build", planEnabled: true }),
    );
    const frame = await h.wait((m) =>
      m.params?.frame?.payload?.deltas?.some((d: any) =>
        d.patch?.pendingInteractions?.some((p: any) => p.payload?.toolName === "ExitPlanMode"),
      ),
    );
    const interaction = frame.params.frame.payload.deltas
      .flatMap((d: any) => d.patch?.pendingInteractions ?? [])
      .find((p: any) => p.payload?.toolName === "ExitPlanMode");
    const after = h.messages.length;
    const reply =
      answer === "approve"
        ? { optionId: "allowOnce" }
        : answer === "feedback"
          ? { freeText: "add tests first" }
          : { optionId: "deny" };
    await h.command(
      h.envelope("resolveInteraction", id, {
        interactionId: interaction.interactionId,
        answer: reply,
      }),
    );
    await h.completed(id, after);
    const rows = (await h.rows(id)).rows as any[];
    const planRow = rows.find((r) => r.kind === "toolCall" && r.toolName === "ExitPlanMode");
    const configs = h.messages
      .flatMap((m) => m.params?.frame?.payload?.deltas ?? [])
      .map((d: any) => d.patch?.config?.planEnabled)
      .filter((v: unknown) => v !== undefined);
    const toolResult = f.requests[1]?.messages.find((m: any) => m.role === "tool")?.content;
    const planFile = await readFile(join(f.cwd, ".zcode", "plans", `plan-${id}.md`), "utf8").catch(
      () => undefined,
    );
    const observation = {
      independentPlanState: capabilities?.independentPlanState ?? false,
      interactionKind: interaction.kind,
      payload: {
        kind: interaction.payload.kind,
        prompt: interaction.payload.prompt,
        freeText: interaction.payload.freeText,
        schema: interaction.payload.schema,
        questions: interaction.payload.questions,
      },
      // 只比较与 plan 有关的消息顺序：系统提示拆分方式、技能 reminder 位置等不在本用例范围。
      order: f.requests.map((r: any) =>
        r.messages
          .map((m: any) => {
            const t = typeof m.content === "string" ? m.content : JSON.stringify(m.content ?? "");
            if (m.role === "tool") return `tool:${t.slice(0, 40)}`;
            if (t.includes("Plan mode is active")) return "reminder:full";
            if (t.includes("Plan mode still active")) return "reminder:sparse";
            if (t.includes("Exited Plan Mode")) return "reminder:exit";
            if (m.role === "user" && (t === "plan it" || t === "add tests first"))
              return `user:${t}`;
            return undefined;
          })
          .filter(Boolean),
      ),
      firstRequestReminders: reminders(f.requests[0]!.messages),
      laterRequestReminders: f.requests.slice(1).map((r: any) => reminders(r.messages)),
      toolResult,
      lastUserText: f.requests
        .at(-1)
        ?.messages.filter((m: any) => m.role === "user")
        .at(-1)?.content,
      requestCount: f.requests.length,
      planRowStatus: planRow?.status,
      finalPlanEnabled: configs.at(-1),
      planFile,
      schemaErrors: h.schemaErrors,
    };
    await h.close();
    return observation;
  } finally {
    await f.close();
  }
}

for (const answer of ["approve", "feedback", "decline"] as const) {
  test(`Node and Rust handle ExitPlanMode ${answer} the same way`, async () => {
    const node = await observePlan("node", answer);
    if (process.env.ZCODE_PLAN_DIFF_DUMP)
      console.log("DUMP", answer, JSON.stringify(node, null, 1));
    const rust = await observePlan("rust", answer);
    assert.deepEqual(rust, node);
  });
}

/** build 模式下模型调用 EnterPlanMode：无需确认、工具结果文案、之后 planEnabled 为 true。 */
async function observeEnter(kind: Runtime) {
  const root = await mkdtemp(join(tmpdir(), `zcode-plan-enter-${kind}-`));
  let step = 0;
  const respond = (_req: any, res: any) => {
    res.writeHead(200, { "content-type": "text/event-stream" });
    if (step++ === 0) return toolCall(res, "EnterPlanMode", {}, "enter-1");
    event(res, { content: "planning" });
    end(res, "stop");
  };
  const f =
    kind === "node"
      ? await fixture({
          root,
          command: process.execPath,
          args: ({ cwd }) => [nodeBundle, "app-server", "--stdio", "--cwd", cwd],
          registry: true,
          respond,
        })
      : await fixture({ root, registry: true, respond });
  try {
    await configureRegistry(f);
    const h = f.start();
    const id = await h.create();
    await h.subscribe(`conversation/${id}`);
    await h.command(h.envelope("sendText", id, { text: "think first", mode: "build" }));
    await h.completed(id);
    const rows = (await h.rows(id)).rows as any[];
    const configs = h.messages
      .flatMap((m) => m.params?.frame?.payload?.deltas ?? [])
      .map((d: any) => d.patch?.config?.planEnabled)
      .filter((v: unknown) => v !== undefined);
    const observation = {
      toolResult: f.requests[1]?.messages.find((m: any) => m.role === "tool")?.content,
      rowStatus: rows.find((r) => r.kind === "toolCall")?.status,
      askedPermission: h.messages.some((m) => JSON.stringify(m).includes('"kind":"permission"')),
      finalPlanEnabled: configs.at(-1),
      schemaErrors: h.schemaErrors,
    };
    await h.close();
    return observation;
  } finally {
    await f.close();
  }
}

test("Node and Rust enter plan mode the same way", async () => {
  const node = await observeEnter("node");
  const rust = await observeEnter("rust");
  assert.deepEqual(rust, node);
});
