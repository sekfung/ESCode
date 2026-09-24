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

// 续传的正确性：base 快照 + 续传增量，经 App 自己的归约器（@zcode/shared applyConversationDeltas）
// 必须得到与同一时刻全新快照相同的状态。两个 runtime 用同一规则检验。
for (const kind of ["node", "rust"] as const) {
  test(`${kind}: base snapshot plus resumed deltas reduces to the fresh snapshot`, async () => {
    const { applyConversationDeltas } = await import("@zcode/shared/zcode-protocol-v4");
    const root = await mkdtemp(join(tmpdir(), `zcode-resume-reduce-${kind}-`));
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
      await h.subscribe(topic, "phone-1", "web-remote-replayable");
      const initial = await h.wait(
        (m) => m.method === "v4/conversation/frame" && m.params?.topic === topic,
      );
      const base = initial.params.frame.payload.snapshot;
      const baseSeq = initial.params.frame.toSeq;
      await h.command(h.envelope("sendText", id, { text: "hello", mode: "yolo" }));
      await h.completed(id);
      const open = async (connectionId: string, withBase: boolean) => {
        const mark = h.messages.length;
        const reply = (await h.client.request(
          "v4/conversation/subscribe",
          {
            topic,
            connectionId,
            clientMode: "web-remote-replayable",
            ...(withBase ? { base: { logEpoch: base.logEpoch, seq: baseSeq } } : {}),
          },
          z.any(),
        )) as any;
        const ack = reply.ack ?? reply;
        return (
          await h.wait(
            (m) =>
              m.method === "v4/conversation/frame" &&
              m.params?.subscriptionId === ack.subscriptionId,
            mark,
          )
        ).params.frame;
      };
      const resumed = await open("phone-2", true);
      const fresh = await open("phone-3", false);
      assert.equal(resumed.payload.kind, "deltas");
      assert.equal(fresh.payload.kind, "snapshot");
      assert.equal(resumed.toSeq, fresh.toSeq, "resume and snapshot describe the same point");
      const reduced = applyConversationDeltas(base, resumed.payload.deltas);
      // seq 由帧携带、快照内另记；两者都指向同一时刻时比较其余全部状态。
      const strip = (s: Record<string, unknown>) => {
        const { seq: _seq, ...rest } = s;
        return rest;
      };
      assert.deepEqual(strip(reduced as any), strip(fresh.payload.snapshot));
      assert.deepEqual(h.schemaErrors, []);
      await h.close();
    } finally {
      await f.close();
    }
  });
}

/** 同一订阅恢复（resync）：带 base 与 forceSnapshot 两种情况的 ACK 与首帧。 */
async function observeResync(kind: Runtime) {
  const root = await mkdtemp(join(tmpdir(), `zcode-resync-${kind}-`));
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
    const sub = (await h.subscribe(topic, "phone-1", "web-remote-replayable")) as any;
    const subscriptionId = (sub.ack ?? sub).subscriptionId;
    const initial = await h.wait(
      (m) => m.method === "v4/conversation/frame" && m.params?.topic === topic,
    );
    const base = initial.params.frame.payload.snapshot;
    const baseSeq = initial.params.frame.toSeq;
    await h.command(h.envelope("sendText", id, { text: "hello", mode: "yolo" }));
    await h.completed(id);
    const resync = async (extra: Record<string, unknown>) => {
      const mark = h.messages.length;
      const reply = (await h.client.request(
        "v4/conversation/resync",
        { subscriptionId, topic, connectionId: "phone-1", ...extra },
        z.any(),
      )) as any;
      const frame = await h.wait(
        (m) => m.method === "v4/conversation/frame" && m.params?.subscriptionId === subscriptionId,
        mark,
      );
      return {
        ackMode: (reply.ack ?? reply).mode,
        payloadKind: frame.params.frame.payload.kind,
        deliveryKind: frame.params.deliveryKind,
        // 只有增量帧的 fromSeq 有意义；快照帧的 fromSeq 取决于各自的建会话初始 seq。
        fromBase:
          frame.params.frame.payload.kind === "deltas"
            ? frame.params.frame.fromSeq === baseSeq
            : null,
      };
    };
    const observation = {
      withBase: await resync({ base: { logEpoch: base.logEpoch, seq: baseSeq } }),
      forced: await resync({
        base: { logEpoch: base.logEpoch, seq: baseSeq },
        forceSnapshot: true,
      }),
      nullBase: await resync({ base: null }),
      schemaErrors: h.schemaErrors,
    };
    await h.close();
    return observation;
  } finally {
    await f.close();
  }
}

test("Node and Rust resync an existing subscription the same way", async () => {
  const node = await observeResync("node");
  if (process.env.ZCODE_RESUME_DUMP) console.log("DUMP resync", JSON.stringify(node));
  const rust = await observeResync("rust");
  if (process.env.ZCODE_RESUME_DUMP) console.log("DUMP-RUST resync", JSON.stringify(rust));
  assert.deepEqual(rust, node);
});

/** 流控：连接 saturated 期间产生增量，drained 后补发的形态（快照还是增量）。 */
async function observeDrain(kind: Runtime) {
  const root = await mkdtemp(join(tmpdir(), `zcode-drain-${kind}-`));
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
    const sub = (await h.subscribe(topic, "phone-1", "web-remote-replayable")) as any;
    const subscriptionId = (sub.ack ?? sub).subscriptionId;
    const initial = await h.wait(
      (m) => m.method === "v4/conversation/frame" && m.params?.topic === topic,
    );
    await h.client.request(
      "v4/connection/flow",
      { connectionId: "phone-1", state: "saturated" },
      z.any(),
    );
    const paused = h.messages.length;
    await h.command(h.envelope("sendText", id, { text: "hello", mode: "yolo" }));
    // 自己的订阅被暂停，用另一连接观察本轮完成。
    await h.subscribe(topic, "observer", "desktop-continuous");
    await h.completed(id, paused);
    const duringPause = h.messages
      .slice(paused)
      .filter(
        (m) => m.method === "v4/conversation/frame" && m.params?.subscriptionId === subscriptionId,
      ).length;
    const mark = h.messages.length;
    await h.client.request(
      "v4/connection/flow",
      { connectionId: "phone-1", state: "drained" },
      z.any(),
    );
    const frame = await h.wait(
      (m) => m.method === "v4/conversation/frame" && m.params?.subscriptionId === subscriptionId,
      mark,
    );
    const observation = {
      framesWhilePaused: duringPause,
      payloadKind: frame.params.frame.payload.kind,
      deliveryKind: frame.params.deliveryKind,
      continuous:
        frame.params.frame.payload.kind === "deltas"
          ? frame.params.frame.fromSeq === initial.params.frame.toSeq
          : null,
      schemaErrors: h.schemaErrors,
    };
    await h.close();
    return observation;
  } finally {
    await f.close();
  }
}

test("Node and Rust resend after a saturated connection drains the same way", async () => {
  const node = await observeDrain("node");
  if (process.env.ZCODE_RESUME_DUMP) console.log("DUMP drain", JSON.stringify(node));
  const rust = await observeDrain("rust");
  if (process.env.ZCODE_RESUME_DUMP) console.log("DUMP-RUST drain", JSON.stringify(rust));
  assert.deepEqual(rust, node);
});
