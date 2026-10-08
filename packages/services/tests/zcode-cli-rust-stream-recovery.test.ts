import assert from "node:assert/strict";
import test from "node:test";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fixture, event, end } from "./zcode-cli-rust-fixture.js";
import { configureRegistry } from "./zcode-cli-rust-registry-fixture.js";

// docs/specs/rust-model-retry.md「断流恢复」：模型已流出推理与部分正文后连接断开，两侧都作废失败尾部
// （正文行 interrupted、推理行 complete）、用同一历史重发请求，并在恢复期间显示同样的 apiRetry 状态。
// 冷恢复是已知差异：Node 重启后把被作废的尝试整段隐藏（与它自己的实时视图不一致），Rust 保持与实时视图相同。
const nodeBundle = resolve("apps/zcode-cli/packages/cli/dist/zcode.cjs");
type Runtime = "node" | "rust";

async function observe(kind: Runtime) {
  const root = await mkdtemp(join(tmpdir(), `zcode-stream-recovery-${kind}-`));
  const bodies: string[] = [];
  const respond = (request: any, response: any) => {
    bodies.push(JSON.stringify(request.messages));
    response.writeHead(200, { "content-type": "text/event-stream" });
    if (bodies.length === 1) {
      event(response, { reasoning_content: "thinking..." });
      event(response, { content: "partial " });
      setTimeout(() => response.destroy(), 300);
      return;
    }
    event(response, { content: "full answer" });
    end(response, "stop");
  };
  const start = () =>
    kind === "node"
      ? fixture({
          root,
          command: process.execPath,
          args: ({ cwd }) => [nodeBundle, "app-server", "--stdio", "--cwd", cwd],
          registry: true,
          respond,
          mode: "yolo",
        })
      : fixture({ root, registry: true, respond, mode: "yolo" });
  const project = (rows: any[]) =>
    rows.map((r) => ({ kind: r.kind, state: r.state ?? null, text: r.text ?? null }));
  try {
    const f = await start();
    let id: string;
    let live: any;
    try {
      await configureRegistry(f);
      const h = f.start();
      id = await h.create();
      await h.subscribe(`conversation/${id}`);
      const mark = h.messages.length;
      await h.command(h.envelope("sendText", id, { text: "hello" }));
      await h.completed(id, mark);
      const retries = h.messages
        .slice(mark)
        .flatMap((m: any) => m.params?.frame?.payload?.deltas ?? [])
        .filter((d: any) => d.patch?.control && "apiRetry" in d.patch.control)
        .map((d: any) => d.patch.control.apiRetry);
      live = {
        rows: project((await h.rows(id)).rows as any[]),
        // 帧数随合并时机不同；比较出现过的恢复态（去掉时间戳）与最终清空。
        recovering: [
          ...new Set(
            retries
              .filter(Boolean)
              .map((r: any) => JSON.stringify({ ...r, nextRetryAt: typeof r.nextRetryAt })),
          ),
        ],
        cleared: retries.at(-1) === null,
      };
      await h.close();
    } finally {
      await f.close();
    }
    const g = await start();
    try {
      await configureRegistry(g);
      const h = g.start();
      await h.subscribe(`conversation/${id}`);
      const cold = project((await h.rows(id)).rows as any[]);
      await h.close();
      return { requests: bodies.length, sameBody: bodies[0] === bodies[1], ...live, cold };
    } finally {
      await g.close();
    }
  } finally {
    await rm(root, { recursive: true, force: true });
  }
}

test("a stream that drops after visible output recovers the same way on both runtimes", async () => {
  const node = await observe("node");
  const rust = await observe("rust");
  // 自检 Node 基准：只重发一次、请求体不含失败尾部，旧正文行作废、新行完成，恢复态出现后被清空。
  assert.equal(node.requests, 2);
  assert.equal(node.sameBody, true);
  assert.deepEqual(
    node.rows.map((r: any) => [r.kind, r.state]),
    [
      ["turnHeader", "completedSuccess"],
      ["userInput", null],
      ["reasoning", "complete"],
      ["assistantText", "interrupted"],
      ["assistantText", "complete"],
    ],
  );
  assert.deepEqual(node.recovering, [
    JSON.stringify({
      attempt: 1,
      maxAttempts: 11,
      nextRetryAt: "number",
      reasonCode: "fault.network.unreachable",
    }),
  ]);
  assert.equal(node.cleared, true);
  // 已知差异（spec）：Node 冷恢复隐藏作废尝试；Rust 冷恢复与实时视图一致。
  assert.deepEqual(
    node.cold.map((r: any) => [r.kind, r.state]),
    [
      ["turnHeader", "completedSuccess"],
      ["userInput", null],
      ["assistantText", "complete"],
    ],
  );
  assert.deepEqual(rust.cold, rust.rows);
  const live = ({ cold: _cold, ...rest }: any) => rest;
  assert.deepEqual(live(rust), live(node));
});
