import assert from "node:assert/strict";
import test from "node:test";
import { resolve } from "node:path";
import type { ServerResponse } from "node:http";
import { conversationTelemetryFactSchema } from "@escode/shared/escode-protocol-v4";
import { fixture, event, type Harness } from "./escode-cli-rust-fixture.js";
import { configureRegistry } from "./escode-cli-rust-registry-fixture.js";
import { canonical, nodeOracle } from "./escode-cli-rust-golden.js";

// `v4/telemetry/event`（TS ConversationTelemetryFactNormalizer）：桌面埋点消费的实时会话事实。一个带工具调用的回合里，
// 两侧发出的事实序列（种类、阶段、计数类字段）一致，且 Rust 的每条事实都通过 shared 的 strict schema。
process.env.ESCODE_TEST_WAIT_MS ??= "60000";
const nodeBundle = resolve("apps/escode-cli/packages/cli/dist/escode.cjs");

function finish(res: ServerResponse, reason: string) {
  res.end(
    `data: ${JSON.stringify({ choices: [{ delta: {}, finish_reason: reason }], usage: { prompt_tokens: 100, completion_tokens: 7, total_tokens: 107 } })}\n\ndata: [DONE]\n\n`,
  );
}

async function observe(kind: "node" | "rust") {
  let n = 0;
  const respond = (_req: any, res: any) => {
    n += 1;
    res.writeHead(200, { "content-type": "text/event-stream" });
    if (n === 1) {
      event(res, { content: "Let me look." });
      event(res, {
        tool_calls: [
          {
            index: 0,
            id: "r-1",
            type: "function",
            function: { name: "TodoRead", arguments: "{}" },
          },
        ],
      });
      finish(res, "tool_calls");
    } else {
      event(res, { content: "Done." });
      finish(res, "stop");
    }
  };
  const f =
    kind === "node"
      ? await fixture({
          command: process.execPath,
          args: ({ cwd }) => [nodeBundle, "app-server", "--stdio", "--cwd", cwd],
          registry: true,
          respond,
          mode: "yolo",
        })
      : await fixture({ registry: true, respond, mode: "yolo" });
  try {
    await configureRegistry(f, false);
    const h: Harness = f.start();
    const sessionId = await h.create();
    await h.subscribe(`conversation/${sessionId}`);
    await h.command(h.envelope("sendText", sessionId, { text: "check todos", mode: "yolo" }));
    await h.completed(sessionId);
    await new Promise((done) => setTimeout(done, 300));
    await h.close();
    const facts = h.messages
      .filter((m: any) => m.method === "v4/telemetry/event" && m.params?.sessionId === sessionId)
      .map((m: any) => m.params);
    const invalid = facts
      .map((fact: any) => conversationTelemetryFactSchema.safeParse(fact))
      .filter((r: any) => !r.success)
      .map((r: any) => r.error.issues);
    const shape = facts
      .filter(
        (fact: any) =>
          !(fact.kind === "model.request.status" && fact.querySource === "session_title"),
      )
      .map((fact: any) => ({
        kind: fact.kind,
        // 会话创建期采用的 App Memory 开关（shared factBaseFields）；两侧都必须带同一取值。
        ...(fact.memoryEnabled !== undefined ? { memoryEnabled: fact.memoryEnabled } : {}),
        ...(fact.phase ? { phase: fact.phase } : {}),
        ...(fact.status ? { status: fact.status } : {}),
        ...(fact.channel ? { channel: fact.channel, firstChunk: fact.firstChunk } : {}),
        ...(fact.toolName ? { toolName: fact.toolName } : {}),
        ...(fact.kind === "usage.delta"
          ? {
              inputTokens: fact.inputTokens,
              outputTokens: fact.outputTokens,
              totalTokens: fact.totalTokens,
            }
          : {}),
        ...(fact.kind === "turn.terminal"
          ? { toolCallCount: fact.toolCallCount, resultType: fact.resultType }
          : {}),
        hasSource: typeof fact.sourceCommandId === "string",
      }));
    return { shape, invalid, schemaErrors: h.schemaErrors };
  } finally {
    await f.close();
  }
}

test("Node and Rust emit the same live conversation telemetry facts", async () => {
  const node = await nodeOracle("observe", () => observe("node"));
  const rust = await canonical(await observe("rust"));
  assert.deepEqual(node.invalid, []);
  assert.deepEqual(rust.invalid, []);
  assert.ok(node.shape.length > 0);
  // 首尾固定（turn.started / turn.terminal）。
  assert.deepEqual(rust.shape[0], node.shape[0]);
  assert.deepEqual(rust.shape.at(-1), node.shape.at(-1));
  // 内容相等：按种类/字段的多重集合比对，避免顺序差异掩盖字段增减。
  const sorted = (shape: unknown[]) => shape.map((s) => JSON.stringify(s)).sort();
  assert.deepEqual(sorted(rust.shape), sorted(node.shape));
  // 顺序相等：不能只比多重集合（docs/specs/rust-unified-host.md §4.6 要求拆成两条断言）。
  // 已批准差异：Node 在同一步里先发工具生命周期、再发 model_request_completed/usage.delta；
  // Rust 在 provider 流结束时结算、随后执行工具。这里显式登记该差异（理由：结算属于模型请求，
  // 工具执行发生在其后；移除条件：TS runtime 删除后或 Rust turn loop 对齐 Node 结算时机时），
  // 把 Node 的每步工具事实移到该步结算之后，再逐条比较顺序；任何额外的顺序变化都会失败。
  assert.deepEqual(rust.shape, rustExpectedOrder(node.shape));
});

/** 把每个模型请求步里的工具生命周期块移到该步结算事实（completed/usage.delta）之后。 */
function rustExpectedOrder(shape: any[]): any[] {
  const isSettlement = (fact: any) =>
    fact.kind === "usage.delta" ||
    (fact.kind === "model.request.status" && fact.status === "model_request_completed");
  const isStepStart = (fact: any) =>
    fact.kind === "model.request.status" && fact.status === "model_request_started";
  const out: any[] = [];
  for (let index = 0; index < shape.length; ) {
    if (!isStepStart(shape[index])) {
      out.push(shape[index++]);
      continue;
    }
    let end = index + 1;
    while (end < shape.length && !isStepStart(shape[end]) && shape[end].kind !== "turn.terminal")
      end++;
    const tools = shape.slice(index, end).filter((fact) => fact.kind === "tool.lifecycle");
    if (!tools.length) {
      out.push(...shape.slice(index, end));
    } else {
      const rest = shape.slice(index, end).filter((fact) => fact.kind !== "tool.lifecycle");
      const settlementAt = rest.findLastIndex(isSettlement);
      // 结算缺失说明该步形态已变化，交给逐条比较报错，不在这里掩盖。
      if (settlementAt < 0) out.push(...rest, ...tools);
      else out.push(...rest.slice(0, settlementAt + 1), ...tools, ...rest.slice(settlementAt + 1));
    }
    index = end;
  }
  return out;
}
