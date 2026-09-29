import assert from "node:assert/strict";
import test from "node:test";
import { join, resolve } from "node:path";
import { mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { end, event, fixture } from "./zcode-cli-rust-fixture.js";
import { configureRegistry } from "./zcode-cli-rust-registry-fixture.js";

// docs/specs/rust-offpeak.md 第三期：会话忙时收到带 `modelExecution` 的输入，TS core admission 直接拒绝
// （不排队、不 steer）。这条用例确认 V4 层回给客户端的 ACK 逐字一致，而不是只确认"被拒"。
process.env.ZCODE_TEST_WAIT_MS ??= "30000";
const nodeBundle = resolve("apps/zcode-cli/packages/cli/dist/zcode.cjs");

async function observe(kind: "node" | "rust") {
  const root = await mkdtemp(join(tmpdir(), `zcode-model-execution-${kind}-`));
  const gate = Promise.withResolvers<void>();
  const started = Promise.withResolvers<void>();
  const f = await fixture({
    root,
    ...(kind === "node"
      ? {
          command: process.execPath,
          args: ({ cwd }) => [nodeBundle, "app-server", "--stdio", "--cwd", cwd],
        }
      : {}),
    registry: true,
    mode: "yolo",
    async respond(_req, res, attempt) {
      if (attempt === 1) {
        started.resolve();
        await gate.promise;
      }
      res.writeHead(200, { "content-type": "text/event-stream" });
      event(res, { content: `answer-${attempt}` });
      end(res, "stop");
    },
  });
  try {
    await configureRegistry(f, false);
    const h = f.start();
    const id = await h.create();
    await h.subscribe(`conversation/${id}`);
    await h.command(h.envelope("sendText", id, { text: "first" }));
    await started.promise;
    // 忙时带单次执行作用域的输入：模型选择与会话当前一致，唯一变量就是 modelExecution。
    const ack = await h.command(
      h.envelope("sendText", id, {
        text: "idle work",
        modelSelection: { providerId: "personal:fixture", modelId: "model-a" },
        modelExecution: { selectionScope: "execution" },
      }),
    );
    gate.resolve();
    await h.completed(id);
    await h.close();
    return {
      status: ack.status,
      reasonCode: (ack as { reasonCode?: string }).reasonCode ?? null,
      message: (ack as { message?: string }).message ?? null,
    };
  } finally {
    gate.resolve();
    await f.close();
  }
}

test("busy modelExecution admission ACK matches Node", async () => {
  const node = await observe("node");
  const rust = await observe("rust");
  assert.ok(node.reasonCode, JSON.stringify(node));
  assert.deepEqual(rust, node, JSON.stringify({ node, rust }));
});
