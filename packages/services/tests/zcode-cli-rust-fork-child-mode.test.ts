import assert from "node:assert/strict";
import test from "node:test";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { z } from "zod";
import { fixture, event, end } from "./zcode-cli-rust-fixture.js";
import { configureRegistry } from "./zcode-cli-rust-registry-fixture.js";

// docs/specs/rust-v4-command-gaps.md「forkAssistant」：fork child 按**被选轮**的协作模式执行
// （TS 取被选 assistant 消息上的 execution state），父会话之后切换模式不影响 child；
// session/read 的 session.mode 与 settings.permission.mode 是同一事实。
// 曾经的缺陷：Node 的 session.mode 取父会话创建时的 permission（显示 build），实际执行却是 yolo；
// Rust 继承父会话当前模式。两侧都已修复，这里逐字比较。
const nodeBundle = resolve("apps/zcode-cli/packages/cli/dist/zcode.cjs");
const any = z.any();
type Runtime = "node" | "rust";

async function observe(kind: Runtime) {
  const root = await mkdtemp(join(tmpdir(), `zcode-fork-mode-${kind}-`));
  const respond = (_request: any, response: any) => {
    response.writeHead(200, { "content-type": "text/event-stream" });
    event(response, { content: "answer" });
    end(response, "stop");
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
    const modes = async (sessionId: string) => {
      const read: any = await h.client.request("session/read", { sessionId }, any);
      return { session: read.session?.mode ?? null, permission: read.settings?.permission?.mode ?? null };
    };
    const parent = await h.create();
    await h.subscribe(`conversation/${parent}`);
    // 第一轮以 yolo 跑，第二轮切回 build：fork 第一轮时 child 应是 yolo，而不是父会话当前的 build。
    let mark = h.messages.length;
    await h.command(h.envelope("sendText", parent, { text: "first", mode: "yolo" }));
    await h.completed(parent, mark);
    mark = h.messages.length;
    await h.command(h.envelope("sendText", parent, { text: "second", mode: "build" }));
    await h.completed(parent, mark);
    const rows = await h.rows(parent);
    const firstReply = (rows.rows as any[]).find((r) => r.kind === "assistantText")!;
    const fork: any = await h.command({
      ...h.envelope("forkAssistant", parent, {
        target: { rowId: firstReply.rowId, entityId: firstReply.entityId },
      }),
      baseRevision: rows.atRevision,
      baseLogEpoch: rows.atLogEpoch,
    });
    const child = fork.result?.sessionId as string;
    const result = {
      ack: fork.status,
      parent: await modes(parent),
      child: await modes(child),
    };
    await h.close();
    return result;
  } finally {
    await f.close();
    await rm(root, { recursive: true, force: true });
  }
}

test("a forked child runs in the mode of the forked turn on both runtimes", async () => {
  const node = await observe("node");
  const rust = await observe("rust");
  // 自检 Node 基准：父会话当前是 build，child 跟被选轮走 yolo，两个字段一致。
  assert.deepEqual(node, {
    ack: "accepted",
    parent: { session: "build", permission: "build" },
    child: { session: "yolo", permission: "yolo" },
  });
  assert.deepEqual(rust, node);
});
