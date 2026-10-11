import assert from "node:assert/strict";
import test from "node:test";
import { dirname, join, resolve } from "node:path";
import { mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { escodeSessionStateSnapshotSchema } from "@escode/shared";
import { fixture, event, end } from "./escode-cli-rust-fixture.js";
import { configureRegistry } from "./escode-cli-rust-registry-fixture.js";

// docs/specs/rust-v4-command-gaps.md「session/read 的 session 投影」：有 Goal 的会话现在也投影
// `session.target`（TS `mapSessionGoal` + `escodeSessionGoalSchema` strict）。给 Goal 补了
// createdAt / updatedAt，并把内部状态（verified / verifying / notSatisfied）归一到协议词表
// （active / paused / budget_limited / complete）。这里设一个 Goal 跑完验证后比较两侧的
// `session.target` 逐字段一致（时间戳比类型与存在性，不比具体值）。
process.env.ESCODE_TEST_WAIT_MS ??= "60000";
type Runtime = "node" | "rust";
const nodeBundle = resolve("apps/escode-cli/packages/cli/dist/escode.cjs");
const UUIDS = /[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/g;

async function observe(kind: Runtime) {
  const root = await mkdtemp(join(tmpdir(), `escode-goal-target-${kind}-`));
  const respond = (_req: any, res: any) => {
    res.writeHead(200, { "content-type": "text/event-stream" });
    const verify = (_req.messages?.at(-1)?.content ?? "").includes("Verify whether the active session goal");
    event(res, {
      content: verify ? '{"passed":true,"reason":"done","nextAction":""}' : "goal work",
    });
    end(res, "stop");
  };
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
          respond,
          mode: "yolo",
          env,
        })
      : await fixture({ root, registry: true, respond, mode: "yolo", env });
  const scrub = (value: string) =>
    value
      .replaceAll(JSON.stringify(root).slice(1, -1), "<root>")
      .replaceAll(root, "<root>")
      .replace(UUIDS, "<uuid>");
  const project = (snapshot: any) => {
    const target = snapshot.session?.target ?? null;
    return JSON.parse(
      scrub(
        JSON.stringify({
          // 无 Goal 的会话是显式 null（既有 session-read-session 用例覆盖）；这里只比有 Goal 的投影。
          hasTarget: snapshot.session?.target !== undefined,
          target: target && {
            sessionIdType: typeof target.sessionId,
            targetIdType: typeof target.targetId,
            objective: target.objective ?? null,
            summaryTitle: target.summaryTitle ?? null,
            status: target.status ?? null,
            tokenBudget: target.tokenBudget ?? null,
            tokensUsedType: typeof target.tokensUsed,
            timeUsedSecondsType: typeof target.timeUsedSeconds,
            activeRunStartedAtMsType: typeof target.activeRunStartedAtMs,
            activeRunLastSeenAtMsType: typeof target.activeRunLastSeenAtMs,
            createdAtType: typeof target.createdAt,
            updatedAtType: typeof target.updatedAt,
          },
        }),
      ),
    ) as Record<string, any>;
  };
  try {
    await configureRegistry(f, false);
    const h = f.start();
    const id = await h.create();
    await h.subscribe(`conversation/${id}`);
    // 设 Goal，跑完一轮验证（verified → complete）。
    await h.command(h.envelope("sendGoalCommand", id, { text: "implement and verify" }));
    await h.wait(
      (m) =>
        m.params?.topic === `conversation/${id}` &&
        m.params.frame?.payload?.deltas?.some((d: any) => d.patch?.goal?.status === "verified"),
    );
    const read: any = await h.client.request(
      "session/read",
      { sessionId: id },
      escodeSessionStateSnapshotSchema,
    );
    const projected = project(read);
    const schemaErrors = h.schemaErrors;
    await h.close();
    if (schemaErrors.length) throw new Error(`${kind}: ${JSON.stringify(schemaErrors)}`);
    return projected;
  } finally {
    await f.close();
  }
}

test("Node and Rust project a goal-bearing session/read target the same way", async () => {
  const node = await observe("node");
  // 自检：确认 Node 侧真的带上 target 且状态归一为 complete，避免两侧同样为空也判等。
  assert.equal(node.hasTarget, true);
  assert.equal(node.target.status, "complete");
  assert.equal(node.target.objective, "implement and verify");
  assert.equal(node.target.createdAtType, "number");
  assert.equal(node.target.updatedAtType, "number");
  const rust = await observe("rust");
  assert.deepEqual(rust, node);
});
