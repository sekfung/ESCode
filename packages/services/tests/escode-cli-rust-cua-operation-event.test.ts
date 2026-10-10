import assert from "node:assert/strict";
import test from "node:test";
import { resolve } from "node:path";
import { escodeComputerUseOperationEventSchema } from "@escode/shared";
import { fixture, event, end, type Harness } from "./escode-cli-rust-fixture.js";
import { configureRegistry } from "./escode-cli-rust-registry-fixture.js";

// `computer-use/operation-event`（Desktop Computer Use 顶部提示的生命周期元数据）：一轮带一次 Bash 调用、随后关闭会话，
// 两侧发出同序的 turn-started / tool-scheduled / tool-started / turn-completed / session-closed。
process.env.ESCODE_TEST_WAIT_MS ??= "60000";
const nodeBundle = resolve("apps/escode-cli/packages/cli/dist/escode.cjs");

async function observe(kind: "node" | "rust") {
  let requests = 0;
  const respond = (_req: any, res: any) => {
    requests += 1;
    res.writeHead(200, { "content-type": "text/event-stream" });
    if (requests === 1) {
      event(res, {
        tool_calls: [{ index: 0, id: "bash-1", type: "function", function: { name: "Bash", arguments: JSON.stringify({ command: "echo ok" }) } }],
      });
      end(res, "tool_calls");
    } else {
      event(res, { content: "done" });
      end(res, "stop");
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
    await h.command(h.envelope("sendText", sessionId, { text: "run", mode: "yolo" }));
    await h.completed(sessionId);
    await h.command(h.envelope("deleteSession", sessionId));
    await new Promise((done) => setTimeout(done, 300));
    await h.close();
    const events = h.messages
      .filter((m: any) => m.method === "computer-use/operation-event")
      .map((m: any) => m.params)
      .filter((p: any) => p.sessionId === sessionId);
    return {
      invalid: events.filter((e: any) => !escodeComputerUseOperationEventSchema.safeParse(e).success).length,
      events: events.map((e: any) => [e.kind, typeof e.turnId, e.toolCallId, e.toolName, e.computerUse]),
    };
  } finally {
    await f.close();
  }
}

test("Node and Rust emit the same Computer Use operation lifecycle", async () => {
  const node = await observe("node");
  const rust = await observe("rust");
  assert.equal(node.invalid, 0);
  assert.equal(rust.invalid, 0);
  assert.deepEqual(rust.events, node.events);
});
