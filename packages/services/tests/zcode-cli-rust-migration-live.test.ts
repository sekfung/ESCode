import assert from "node:assert/strict";
import test from "node:test";
import { join, resolve } from "node:path";
import { mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { fixture } from "./zcode-cli-rust-fixture.js";
import { configureRegistry } from "./zcode-cli-rust-registry-fixture.js";
import { zcodeSessionStateSnapshotSchema } from "@zcode/shared";
import { DatabaseSync } from "node:sqlite";

const nodeBundle = resolve("apps/zcode-cli/packages/cli/dist/zcode.cjs");

// docs/specs/rust-release-rollback.md「数据迁移」：用真实 Node runtime 写出的 TS 数据做导入验收，
// 而不是手写 fixture——这是「真实用户数据」之外能拿到的最接近的口径。
test("Rust imports a session written by the real Node runtime and preserves its content", async () => {
  // 1) Node runtime 在 fixture 的临时目录里真实跑一轮：会话 + 用户消息 + Write 工具调用 + 助手回复。
  // 自己持有 root：Node 阶段结束后数据要留给 Rust 导入（fixture 只清理自己创建的目录）。
  const root = await mkdtemp(join(tmpdir(), "zcode-ts-live-"));
  const nodeFixture = await fixture({
    root,
    command: process.execPath,
    args: ({ cwd }) => [nodeBundle, "app-server", "--stdio", "--cwd", cwd],
    registry: true,
  });
  let sessionId = "";
  let nodeHarness: ReturnType<typeof nodeFixture.start> | undefined;
  try {
    await configureRegistry(nodeFixture);
    const h = nodeFixture.start();
    nodeHarness = h;
    sessionId = await h.create();
    await h.subscribe(`conversation/${sessionId}`);
    // 显式 yolo：默认 build 下 Write 会等用户确认，这里要的是「Node 真实写出的工具行」。
    await h.command(h.envelope("sendText", sessionId, { text: "write", mode: "yolo" }));
    await h.completed(sessionId);
    await h.close();
  } finally {
    await nodeFixture.close();
  }

  // TS 侧确实落库了内容（否则后面的导入断言没有意义）。
  const ts = new DatabaseSync(join(root, "ts.sqlite"), { readOnly: true });
  let tsMessages = 0;
  try {
    tsMessages = (
      ts.prepare("SELECT COUNT(*) AS n FROM message WHERE session_id=?").get(sessionId) as {
        n: number;
      }
    ).n;
  } finally {
    ts.close();
  }
  assert(tsMessages >= 2, `Node runtime wrote ${tsMessages} messages`);

  // 2) Rust runtime 导入同一份数据，内容必须逐项对上。
  const rustFixture = await fixture({ root, legacy: true });
  let rustHarness: ReturnType<typeof rustFixture.start> | undefined;
  try {
    const h = rustFixture.start();
    rustHarness = h;
    await h.subscribe(`conversation/${sessionId}`);
    const snapshot = await h.client.request(
      "session/read",
      { sessionId },
      zcodeSessionStateSnapshotSchema,
    );
    assert.equal(snapshot.session.title, "write");
    // 行级事实：用户输入、工具调用、助手回复都要在，且与 Node 写的一致。
    const rows = (await h.rows(sessionId)).rows;
    const kinds = rows.map((r) => r.kind);
    assert(kinds.includes("turnHeader"), `imported rows missing turn header: ${kinds.join(",")}`);
    assert(
      rows.some((r) => r.kind === "userInput" && (r as { text?: string }).text === "write"),
      "imported rows lost the user input written by the Node runtime",
    );
    assert(
      rows.some((r) => r.kind === "toolCall" && (r as { toolName?: string }).toolName === "Write"),
      "imported rows lost the Write tool call from the Node runtime",
    );
    assert(
      rows.some((r) => r.kind === "assistantText"),
      "imported rows lost the assistant reply from the Node runtime",
    );
    await h.close();
    assert.deepEqual(rustHarness?.schemaErrors, []);
    assert.deepEqual(nodeHarness?.schemaErrors, []);
  } finally {
    await rustFixture.close();
  }
});
