import assert from "node:assert/strict";
import test from "node:test";
import { join, resolve } from "node:path";
import { mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { z } from "zod";
import { fixture, event, end } from "./escode-cli-rust-fixture.js";
import { configureRegistry } from "./escode-cli-rust-registry-fixture.js";

// docs/specs/rust-offpeak.md 第三期：执行作用域（modelExecution）——本轮用输入的 modelSelection 与单次凭据
// （off-peak 账号模型不向 Host 请求 header），会话模型不变；子代理后台拒绝、前台沿用本轮模型与凭据。
process.env.ESCODE_TEST_WAIT_MS ??= "30000";
const nodeBundle = resolve("apps/escode-cli/packages/cli/dist/escode.cjs");
const text = (m: any) => (typeof m.content === "string" ? m.content : JSON.stringify(m.content));
const execution = {
  selectionScope: "execution",
  memoryExtraction: "skip",
  requestAuth: { headers: { "X-Idle": "ticket-1" } },
  subagents: { foregroundModel: "submission", background: "deny" },
};
const calls = [
  {
    name: "Agent",
    arguments: { description: "bg", prompt: "look around", run_in_background: true },
  },
  { name: "Agent", arguments: { description: "fg", prompt: "look around" } },
];

async function observe(kind: "node" | "rust") {
  const root = await mkdtemp(join(tmpdir(), `escode-offpeak-exec-${kind}-`));
  const main: { model: string; idle: unknown; results: string[] }[] = [];
  const child: { model: string; idle: unknown }[] = [];
  let f!: Awaited<ReturnType<typeof fixture>>;
  const respond = (req: any, res: any, attempt: number) => {
    res.writeHead(200, { "content-type": "text/event-stream" });
    const first = text(req.messages[0] ?? {});
    if (
      req.stream === false ||
      req.stream === undefined ||
      first.startsWith("Generate a concise title")
    ) {
      event(res, { content: "Title" });
      end(res, "stop");
      return;
    }
    const idle = f.requestHeaders[attempt - 1]?.["x-idle"];
    const isChild =
      req.messages.some((m: any) => text(m).includes("look around")) &&
      !req.messages.some((m: any) => text(m).includes("queue idle work"));
    if (isChild) {
      child.push({ model: `${req.model}/${req.reasoning_effort}`, idle });
      event(res, { content: "child done" });
      end(res, "stop");
      return;
    }
    const results = req.messages
      .filter((m: any) => m.role === "tool")
      .map((m: any) =>
        text(m)
          .replace(/agent_[0-9a-f-]+/g, "agent_<id>")
          .replace(/duration_ms: \d+/g, "duration_ms: <n>"),
      );
    main.push({ model: `${req.model}/${req.reasoning_effort}`, idle, results });
    const next = calls[results.length];
    if (next) {
      event(res, {
        tool_calls: [
          {
            index: 0,
            id: `call-${results.length}`,
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
  f =
    kind === "node"
      ? await fixture({
          root,
          command: process.execPath,
          args: ({ cwd }) => [nodeBundle, "app-server", "--stdio", "--cwd", cwd],
          registry: true,
          respond,
          mode: "yolo",
        })
      : await fixture({ root, registry: true, respond, mode: "yolo" });
  try {
    const { revision } = await configureRegistry(f, true, { accountMode: "off-peak" });
    const h = f.start();
    let hostAuth = 0;
    h.hostHandlers = {
      "interaction/requestProviderRuntimeHeaders": () => {
        hostAuth++;
        return { headersApplied: true, requestAuth: { headers: { "X-Host": "h" } } };
      },
    };
    await h.client.request(
      "provider/updateAccountConfig",
      {
        revision: "offpeak",
        basedOnESCodeBuiltinRevision: revision,
        providers: { "account:fixture": { access: { type: "zhipu-account", entitled: true } } },
        states: { "account:fixture": { current: true, entitled: true, availability: "available" } },
      },
      z.any(),
    );
    const id = await h.create();
    await h.subscribe(`conversation/${id}`);
    await h.command(
      h.envelope("sendText", id, {
        text: "queue idle work",
        mode: "yolo",
        modelSelection: {
          providerId: "account:fixture",
          modelId: "model-a",
          options: { reasoningLevel: "high" },
        },
        modelExecution: execution,
        offPeakTaskId: "offpeak-9",
        offPeakRunType: "init",
      }),
    );
    await h.completed(id);
    const read: any = await h.client.request("session/read", { sessionId: id }, z.any());
    await h.close();
    // 凭据不出现在协议输出中。
    const leaked = JSON.stringify(h.messages).includes("ticket-1");
    // 会话模型选择不被执行作用域改写（仍为 low）。
    const selection = JSON.stringify(read).match(/"reasoningLevel":"(low|high)"/)?.[1];
    return { main, child, hostAuth, leaked, selection };
  } finally {
    await f.close();
  }
}

test("OffPeak execution-scoped model, credentials and subagent policy match Node", async () => {
  const node = await observe("node");
  const rust = await observe("rust");
  // 自检：Node 用执行模型与本轮凭据，不向 Host 请求 header，会话模型不变。
  assert.ok(node.main.length >= 2, JSON.stringify(node.main));
  assert.ok(
    node.main.every((r) => r.model === "model-a/high" && r.idle === "ticket-1"),
    JSON.stringify(node.main),
  );
  assert.equal(node.hostAuth, 0);
  assert.equal(node.leaked, false);
  assert.match(node.main[1]!.results[0]!, /do not support background agents/);
  assert.deepEqual(rust.main, node.main);
  assert.deepEqual(rust.child, node.child);
  assert.equal(rust.hostAuth, node.hostAuth);
  assert.equal(rust.leaked, false);
  assert.equal(rust.selection, node.selection);
});
