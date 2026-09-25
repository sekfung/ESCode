import assert from "node:assert/strict";
import test from "node:test";
import { join, resolve } from "node:path";
import { mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { zcodeSessionListResultSchema } from "@zcode/shared";
import { fixture, event, end } from "./zcode-cli-rust-fixture.js";
import { configureRegistry } from "./zcode-cli-rust-registry-fixture.js";

// docs/specs/rust-cron.md：同一段对话在 Node 与 Rust 上创建/列出/更新/删除定时任务，
// Host 收到的请求、模型看到的结果与会话标题一致；自动化轮中写工具不可见且被拒绝。
const nodeBundle = resolve("apps/zcode-cli/packages/cli/dist/zcode.cjs");
const text = (m: any) => (typeof m.content === "string" ? m.content : JSON.stringify(m.content));

const automation = {
  automationId: "auto_1",
  title: "每天9点 digest",
  cronExpr: "0 9 * * *",
  prompt: "digest",
  enabled: true,
  lifecycleStatus: "active",
  nextRunAt: 1767000000000,
  runCount: 0,
  recurring: true,
  modelSelection: {
    providerId: "personal:fixture",
    modelId: "model-a",
    options: { reasoningLevel: "low" },
  },
  mode: "build",
  scheduleRule: { unit: "daily", interval: 1, hour: 9, minute: 0, anchorAt: 1766000000000 },
};

const calls = [
  {
    name: "CronCreate",
    arguments: { cron: "0 9 * * *", prompt: " digest ", title: "每天9点 digest" },
  },
  { name: "CronList", arguments: {} },
  { name: "CronUpdate", arguments: { id: "auto_1", title: "每天10点 digest", cron: "0 10 * * *" } },
  { name: "CronDelete", arguments: { id: "auto_1" } },
];

async function observe(kind: "node" | "rust") {
  const root = await mkdtemp(join(tmpdir(), `zcode-cron-${kind}-`));
  const requests: any[] = [];
  const respond = (req: any, res: any) => {
    if (req.stream === false || req.stream === undefined) {
      res.writeHead(200, { "content-type": "application/json" });
      res.end(
        JSON.stringify({
          id: "t",
          object: "chat.completion",
          choices: [
            { index: 0, message: { role: "assistant", content: "Title" }, finish_reason: "stop" },
          ],
          usage: { prompt_tokens: 1, completion_tokens: 1, total_tokens: 2 },
        }),
      );
      return;
    }
    if (text(req.messages[0] ?? {}).startsWith("Generate a concise title")) {
      res.writeHead(200, { "content-type": "text/event-stream" });
      event(res, { content: "Title" });
      end(res, "stop");
      return;
    }
    requests.push(req);
    res.writeHead(200, { "content-type": "text/event-stream" });
    const automationTurn = req.messages.some((m: any) => text(m).includes("scheduled run"));
    const done = req.messages.filter((m: any) => m.role === "tool").length;
    const next = automationTurn
      ? req.messages.at(-1)?.role === "tool"
        ? undefined
        : { name: "CronCreate", arguments: { cron: "0 9 * * *", prompt: "p", title: "t" } }
      : calls[done];
    if (next) {
      event(res, {
        tool_calls: [
          {
            index: 0,
            id: `call-${done}`,
            type: "function",
            function: { name: next.name, arguments: JSON.stringify(next.arguments) },
          },
        ],
      });
      end(res, "tool_calls");
      return;
    }
    event(res, { content: "done" });
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
    h.hostHandlers = {
      "automation/checkTaskBinding": () => ({ bound: false }),
      "automation/create": () => ({ automation }),
      "automation/list": () => ({ automations: [automation] }),
      "automation/update": () => ({
        automation: { ...automation, title: "每天10点 digest", cronExpr: "0 10 * * *" },
      }),
      "automation/delete": () => ({ deleted: true }),
    };
    const id = await h.create();
    await h.subscribe(`conversation/${id}`);
    await h.command(h.envelope("sendText", id, { text: "manage my digest", mode: "yolo" }));
    await h.completed(id);
    const listed = (await h.client.request(
      "session/list",
      {},
      zcodeSessionListResultSchema,
    )) as any;
    const title = listed.sessions.find((s: any) => s.sessionId === id)?.title;
    const manualTools = requests
      .at(-1)
      .messages.filter((m: any) => m.role === "tool")
      .map((m: any) => text(m).replaceAll(id, "<session>"));
    const hostManual = h.hostRequests.map((r) =>
      JSON.parse(JSON.stringify(r).replaceAll(id, "<session>")),
    );
    // 自动化派发轮：Host 以 automationId 发送输入。
    const other = await h.create();
    await h.subscribe(`conversation/${other}`);
    const before = requests.length;
    await h.command(
      h.envelope("sendText", other, {
        text: "scheduled run",
        mode: "yolo",
        automationId: "auto_9",
      }),
    );
    await h.completed(other);
    const automationRequest = requests[before];
    // 官方插件 MCP 工具取决于插件 seed 环境（另有用例覆盖），这里只比对内置工具面。
    const automationTools = (automationRequest?.tools ?? [])
      .map((t: any) => t.function?.name ?? t.name)
      .filter((name: string) => !name.startsWith("mcp__"));
    const automationResult = requests
      .slice(before)
      .flatMap((r: any) => r.messages)
      .find((m: any) => m.role === "tool");
    const observation = {
      title,
      manualTools,
      hostManual,
      automationTools,
      automationResult: automationResult ? text(automationResult) : undefined,
      schemaErrors: h.schemaErrors,
    };
    await h.close();
    return observation;
  } finally {
    await f.close();
  }
}

test("Node and Rust manage scheduled tasks the same way", async () => {
  const node = await observe("node");
  const rust = await observe("rust");
  assert.deepEqual(node.schemaErrors, []);
  assert.deepEqual(rust.schemaErrors, []);
  // 自检：Node 走完四个工具、Host 应答通过协议校验，且冻结了标题。
  assert.equal(node.manualTools.length, 4);
  assert.ok(node.manualTools[0].startsWith('{"automation"'));
  assert.equal(node.title, "每天9点 digest");

  assert.deepEqual(rust.hostManual, node.hostManual);
  assert.deepEqual(rust.manualTools, node.manualTools);
  assert.equal(rust.title, node.title);
  // 自动化轮：写工具从工具面移除；模型仍调用时两侧都拒绝（错误包装格式各自不同，只比对原因）。
  for (const tool of ["CronCreate", "CronUpdate", "CronDelete"]) {
    assert.ok(!node.automationTools.includes(tool), `node hides ${tool}`);
    assert.ok(!rust.automationTools.includes(tool), `rust hides ${tool}`);
  }
  assert.ok(node.automationTools.includes("CronList"));
  assert.deepEqual(rust.automationTools, node.automationTools);
  assert.ok(node.automationResult?.includes("not allowed while running a scheduled automation"));
  assert.ok(rust.automationResult?.includes("not allowed while running a scheduled automation"));
});
