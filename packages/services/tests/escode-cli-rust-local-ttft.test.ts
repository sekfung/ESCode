import assert from "node:assert/strict";
import test from "node:test";
import { randomUUID } from "node:crypto";
import { resolve } from "node:path";
import { localTtftFactsSchema } from "@escode/shared";
import { fixture, event, end, type Harness } from "./escode-cli-rust-fixture.js";
import { configureRegistry } from "./escode-cli-rust-registry-fixture.js";

// 本地首 token 时延观测（TS LocalTtftRecorder）：命令信封带 `ttft` 时，`v4/telemetry/local-ttft` 检查点按
// 接收 → 准入 → 起跑 → 首请求 → 首输出推进；`v4/commands/query {clock}` 回时钟探测。比对检查点的字段集合与关键值。
process.env.ESCODE_TEST_WAIT_MS ??= "60000";
const nodeBundle = resolve("apps/escode-cli/packages/cli/dist/escode.cjs");

async function observe(kind: "node" | "rust") {
  const respond = (_req: any, res: any) => {
    res.writeHead(200, { "content-type": "text/event-stream" });
    event(res, { content: "Hello there." });
    end(res, "stop");
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
    const parse = { parse: (value: unknown) => value } as any;
    const clock = (await h.client.request(
      "v4/commands/query",
      { commands: [{ sessionId: null, commandId: "probe" }], clock: true },
      parse,
    )) as any;
    const observationId = randomUUID();
    const envelope = { ...h.envelope("sendText", sessionId, { text: "hello", mode: "yolo" }), ttft: { version: 1, observationId } };
    const ack = await h.command(envelope as any);
    await h.completed(sessionId);
    await new Promise((done) => setTimeout(done, 300));
    await h.close();
    const checkpoints = h.messages
      .filter((m: any) => m.method === "v4/telemetry/local-ttft")
      .map((m: any) => m.params);
    const last = checkpoints.at(-1) ?? {};
    // 首输出只经在线增量帧的 `ttft` 送达（检查点里没有 outputAt）。
    const framed = h.messages
      .map((m: any) => m.params?.frame?.ttft)
      .filter((t: any) => t && t.observationId === observationId)
      .at(-1) ?? {};
    // TS 内部的准备阶段计时（context / hooks / persistence / …）与 logicalCallId / queryId 是 Node 实现细节，不比对。
    const internal = new Set(["logicalCallId", "queryId"]);
    return {
      clockKeys: Object.keys(clock.clock ?? {}).sort(),
      clockResults: clock.results?.map((r: any) => r.result),
      ackExcluded: (ack as any).ttftExcluded ?? null,
      invalid: checkpoints.filter((c: any) => !localTtftFactsSchema.safeParse(c).success).length,
      lastKeys: Object.keys(last).filter((k) => !internal.has(k)).sort(),
      framedKeys: Object.keys(framed).filter((k) => !internal.has(k)).sort(),
      framedOutput: framed.outputKind,
      framedOrdered: framed.receivedAt <= framed.admittedAt && framed.admittedAt <= framed.requestAt && framed.requestAt <= framed.outputAt,
      lastValues: {
        observationId: last.observationId === observationId,
        sendMode: last.sendMode,
        outputKind: last.outputKind,
        model: last.model,
        provider: last.provider,
        stages: (framed.details ?? [])
          .filter((d: any) => d.stage === "attempt" || d.stage === "retry_wait")
          .map((d: any) => [d.stage, d.role, d.outcome, d.source]),
      },
    };
  } finally {
    await f.close();
  }
}

test("Node and Rust observe local time-to-first-token the same way", async () => {
  const node = await observe("node");
  const rust = await observe("rust");
  assert.equal(node.invalid, 0);
  assert.equal(node.framedOutput, "text");
  for (const key of Object.keys(node)) {
    assert.deepEqual((rust as any)[key], (node as any)[key], key);
  }
});
