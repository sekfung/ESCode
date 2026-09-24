import assert from "node:assert/strict";
import test from "node:test";
import { join, resolve } from "node:path";
import { mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { z } from "zod";
import { fixture } from "./zcode-cli-rust-fixture.js";
import { configureRegistry } from "./zcode-cli-rust-registry-fixture.js";

// WP6：手机远控断线重连时带 base 续订，应拿到 base 之后的增量（resume），而不是整份快照。
const nodeBundle = resolve("apps/zcode-cli/packages/cli/dist/zcode.cjs");
type Runtime = "node" | "rust";

async function observeResume(kind: Runtime, clientMode: string) {
  const root = await mkdtemp(join(tmpdir(), `zcode-resume-${kind}-`));
  const f =
    kind === "node"
      ? await fixture({
          root,
          command: process.execPath,
          args: ({ cwd }) => [nodeBundle, "app-server", "--stdio", "--cwd", cwd],
          registry: true,
        })
      : await fixture({ root, registry: true });
  try {
    await configureRegistry(f);
    const h = f.start();
    const id = await h.create();
    const topic = `conversation/${id}`;
    await h.subscribe(topic, "phone-1", clientMode);
    const frames = () =>
      h.messages.filter((m) => m.method === "v4/conversation/frame" && m.params?.topic === topic);
    const initial = await h.wait(
      (m) => m.method === "v4/conversation/frame" && m.params?.topic === topic,
    );
    const logEpoch = initial.params.frame.payload.snapshot?.logEpoch ?? initial.params.logEpoch;
    const baseSeq = initial.params.frame.toSeq;
    await h.command(h.envelope("sendText", id, { text: "hello", mode: "yolo" }));
    await h.completed(id);
    const lastSeq = frames().at(-1)!.params.frame.toSeq;
    const mark = h.messages.length;
    const reply = (await h.client.request(
      "v4/conversation/subscribe",
      { topic, connectionId: "phone-2", clientMode, base: { logEpoch, seq: baseSeq } },
      z.any(),
    )) as any;
    const ack = reply.ack ?? reply;
    const resumed = await h.wait(
      (m) =>
        m.method === "v4/conversation/frame" &&
        m.params?.topic === topic &&
        m.params?.subscriptionId === ack.subscriptionId,
      mark,
    );
    const observation = {
      ackMode: ack.mode,
      payloadKind: resumed.params.frame.payload.kind,
      deliveryKind: resumed.params.deliveryKind,
      fromSeq: resumed.params.frame.fromSeq - baseSeq,
      reachesLatest: resumed.params.frame.toSeq === lastSeq,
      // 续传必须恰好覆盖 (base, current]：seq 与增量一一对应。
      coversRange:
        (resumed.params.frame.payload.deltas ?? []).length ===
        resumed.params.frame.toSeq - resumed.params.frame.fromSeq,
      // 流式发布粒度不同（Rust 以 row.delta 推送文本分片），只比较变更种类。
      opKinds: [
        ...new Set(
          (resumed.params.frame.payload.deltas ?? [])
            .map((d: any) => d.op)
            .filter((op: string) => op !== "row.delta"),
        ),
      ].sort(),
      schemaErrors: h.schemaErrors,
    };
    await h.close();
    return observation;
  } finally {
    await f.close();
  }
}

for (const clientMode of ["web-remote-replayable", "desktop-continuous"] as const) {
  test(`Node and Rust resume a ${clientMode} subscription from base the same way`, async () => {
    const node = await observeResume("node", clientMode);
    if (process.env.ZCODE_RESUME_DUMP) console.log("DUMP", clientMode, JSON.stringify(node));
    const rust = await observeResume("rust", clientMode);
    if (process.env.ZCODE_RESUME_DUMP) console.log("DUMP-RUST", clientMode, JSON.stringify(rust));
    assert.deepEqual(rust, node);
  });
}

// base 来自旧 logEpoch（例如 runtime 重启前）：不能伪造连续水位，两侧都回落 snapshot。
test("Node and Rust fall back to a snapshot when the base epoch is stale", async () => {
  const observe = async (kind: Runtime) => {
    const root = await mkdtemp(join(tmpdir(), `zcode-resume-stale-${kind}-`));
    const f =
      kind === "node"
        ? await fixture({
            root,
            command: process.execPath,
            args: ({ cwd }) => [nodeBundle, "app-server", "--stdio", "--cwd", cwd],
            registry: true,
          })
        : await fixture({ root, registry: true });
    try {
      await configureRegistry(f);
      const h = f.start();
      const id = await h.create();
      const topic = `conversation/${id}`;
      const mark = h.messages.length;
      const reply = (await h.client.request(
        "v4/conversation/subscribe",
        {
          topic,
          connectionId: "phone-1",
          clientMode: "web-remote-replayable",
          base: { logEpoch: "stale-epoch", seq: 0 },
        },
        z.any(),
      )) as any;
      const ack = reply.ack ?? reply;
      const frame = await h.wait(
        (m) => m.method === "v4/conversation/frame" && m.params?.topic === topic,
        mark,
      );
      const observation = {
        ackMode: ack.mode,
        payloadKind: frame.params.frame.payload.kind,
        schemaErrors: h.schemaErrors,
      };
      await h.close();
      return observation;
    } finally {
      await f.close();
    }
  };
  const node = await observe("node");
  const rust = await observe("rust");
  assert.deepEqual(rust, node);
  assert.equal(rust.ackMode, "snapshot");
});
