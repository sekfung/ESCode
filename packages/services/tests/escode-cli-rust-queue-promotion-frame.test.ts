import assert from "node:assert/strict";
import test from "node:test";
import { dirname, join, resolve } from "node:path";
import { mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import type { ConversationTopicFrame } from "@escode/shared/escode-protocol-v4";
import { fixture } from "./escode-cli-rust-fixture.js";
import { configureRegistry } from "./escode-cli-rust-registry-fixture.js";
import { createBotConversationStream } from "../src/bots/botConversationStream.js";

// docs/specs/bots-v4-stream.md §4：Bot 投影把“排队项消失但没见到它的轮头”判为丢弃，前提是
// 提升执行时队列补丁与新轮头在同一帧。这里在 Node（TS runtime）与 Rust 上各跑一次
// “慢请求占住 → 排队一条 → 被提升”，直接检查原始帧，并确认投影不误报丢弃。
process.env.ESCODE_TEST_WAIT_MS ??= "60000";
type Runtime = "node" | "rust";
const nodeBundle = resolve("apps/escode-cli/packages/cli/dist/escode.cjs");

async function observe(kind: Runtime) {
  const root = await mkdtemp(join(tmpdir(), `escode-queue-frame-${kind}-`));
  const env = {
    ESCODE_OFFICIAL_PLUGINS_BASE_DIR: dirname(nodeBundle),
    ESCODE_PLUGIN_HOST_EXEC_PATH: process.execPath,
    ESCODE_PLUGIN_HOST_ENTRYPOINT: nodeBundle,
  };
  const f =
    kind === "node"
      ? await fixture({
          root,
          command: process.execPath,
          args: ({ cwd }) => [nodeBundle, "app-server", "--stdio", "--cwd", cwd],
          registry: true,
          mode: "yolo",
          env,
        })
      : await fixture({ root, registry: true, mode: "yolo" });
  try {
    await configureRegistry(f, false);
    const h = f.start();
    const id = await h.create();
    const sub = await h.subscribe(`conversation/${id}`);
    const before = h.messages.length;
    await h.command(h.envelope("sendText", id, { text: "slow" }));
    const queued = h.envelope("sendText", id, { text: "queued", requestedDelivery: "queue" });
    await h.command(queued);
    // 等排队输入的轮次完成（两轮都结束）。
    await h.wait(
      (m) =>
        m.params?.subscriptionId === sub.ack.subscriptionId &&
        (m.params?.frame?.payload?.deltas ?? []).some(
          (d: any) =>
            d.row?.kind === "turnHeader" &&
            d.row.sourceCommandId === queued.commandId &&
            d.row.state === "completedSuccess",
        ),
      before,
    );
    const frames: ConversationTopicFrame[] = h.messages
      .filter((m) => m.params?.subscriptionId === sub.ack.subscriptionId && m.params?.frame)
      .map((m) => m.params.frame);
    const schemaErrors = h.schemaErrors;
    await h.close();
    if (schemaErrors.length) throw new Error(`${kind}: ${JSON.stringify(schemaErrors)}`);
    return { frames, queuedId: queued.commandId };
  } finally {
    await f.close();
  }
}

type ConversationDelta = Extract<
  ConversationTopicFrame["payload"],
  { kind: "deltas" }
>["deltas"][number];
type StateUpdatedDelta = Extract<ConversationDelta, { op: "state.updated" }>;

function analyse(kind: Runtime, frames: ConversationTopicFrame[], queuedId: string) {
  const inQueue = (frame: ConversationTopicFrame): boolean | undefined => {
    // findLast 不像 filter 会自动收窄；用显式谓词把 state.updated 的 patch 带出来。
    const queueUpdate =
      frame.payload.kind === "deltas"
        ? frame.payload.deltas.findLast(
            (d): d is StateUpdatedDelta => d.op === "state.updated" && Boolean(d.patch.queue),
          )
        : undefined;
    const items =
      frame.payload.kind === "snapshot"
        ? frame.payload.snapshot.queue.items
        : queueUpdate?.patch.queue?.items;
    return items?.some((item) => item.sourceCommandId === queuedId);
  };
  const hasTurn = (frame: ConversationTopicFrame) =>
    frame.payload.kind === "deltas" &&
    frame.payload.deltas.some(
      (d) =>
        (d.op === "row.appended" || d.op === "row.upserted") &&
        (d.row.kind === "turnHeader" || d.row.kind === "userInput") &&
        d.row.sourceCommandId === queuedId,
    );
  let wasQueued = false;
  let removal: number | undefined;
  frames.forEach((frame, index) => {
    const present = inQueue(frame);
    if (present === true) wasQueued = true;
    if (present === false && wasQueued && removal === undefined) removal = index;
  });
  assert.equal(wasQueued, true, `${kind}: input was never queued`);
  assert.notEqual(removal, undefined, `${kind}: queued input never left the queue`);
  const firstTurn = frames.findIndex(hasTurn);
  // 同帧（或轮头更早）才能让投影把它判为“已执行”。
  assert.ok(
    firstTurn !== -1 && firstTurn <= removal!,
    `${kind}: queue removal at frame ${removal}, first turn row at frame ${firstTurn}`,
  );
  const projection = createBotConversationStream("task");
  const events = frames.flatMap((frame) => projection.accept(frame));
  assert.deepEqual(
    events.filter((event) => event.type === "turn_steer_status"),
    [],
    `${kind}: promoted input reported as discarded`,
  );
  return { removal: removal!, firstTurn };
}

for (const kind of ["node", "rust"] as const) {
  test(`${kind} runtime promotes a queued input in the same frame as its turn`, async () => {
    const { frames, queuedId } = await observe(kind);
    analyse(kind, frames, queuedId);
  });
}
