import assert from "node:assert/strict";
import test from "node:test";
import { dirname, join, resolve } from "node:path";
import { mkdtemp } from "node:fs/promises";
import { availableParallelism, tmpdir } from "node:os";
import { fixture, event, end, type Harness } from "./zcode-cli-rust-fixture.js";
import { configureRegistry } from "./zcode-cli-rust-registry-fixture.js";

// 修复（CI macOS 偶发失败）：run 的并发天花板是 `max(1, min(16, availableParallelism()-2))`（TS
// workflow-concurrency-ceiling.ts）。3 核的 macOS runner 上天花板就是 1，就地调到 1 会被读成 unchanged，
// 不落设置轮，用例一直等第四行直到超时。天花板为 1 时没有更低的界可调，此时只校验两侧同样拒绝。
const CAN_RETUNE = Math.max(1, Math.min(16, availableParallelism() - 2)) > 1;
const SETTINGS_ROWS = CAN_RETUNE ? 4 : 2;

// docs/specs/rust-v4-command-gaps.md「amendWorkflowRunSettings」：run 卡 / 详情页「配置」的「应用」在
// Node 与 Rust 上给出一致的命令 ACK（修订出新 run 的 supersededRunId、就地调并发、unchanged /
// not_found / not_configurable / model_unavailable 四条拒绝）与设置轮两行（turnHeader + userInput，
// origin = workflowLaunch）。修订沿用前驱脚本、继承实参，由 core `applyWorkflowRunSettings` 共用同一段
// 实现；Rust 只做模型目录（补 current）与落设置轮。
process.env.ZCODE_TEST_WAIT_MS ??= "60000";
type Message = Record<string, any>;
const nodeBundle = resolve("apps/zcode-cli/packages/cli/dist/zcode.cjs");
const script = [
  "interface Answer { total: number; }",
  'const counter = agent("counter", { system: "You count things carefully." });',
  'const answer = await counter.ask<Answer>("How many items are there?");',
  "return { total: answer.total };",
].join("\n");

const call = (id: string, name: string, args: unknown) => ({
  tool_calls: [
    { index: 0, id, type: "function", function: { name, arguments: JSON.stringify(args) } },
  ],
});
const toolResult = (req: any, id: string) =>
  String(
    req?.messages?.find((m: any) => m.role === "tool" && m.tool_call_id === id)?.content ?? "",
  );
const until = async (condition: () => boolean | Promise<boolean>, ms = 60_000) => {
  const deadline = Date.now() + ms;
  while (!(await condition()) && Date.now() < deadline) {
    await new Promise((done) => setTimeout(done, 50));
  }
  return condition();
};

async function observe(kind: "node" | "rust") {
  const root = await mkdtemp(join(tmpdir(), `zcode-amend-settings-${kind}-`));
  const main: any[] = [];
  const actor: any[] = [];
  const hungRes: any[] = [];
  let runId = "missing";
  const respond = (req: any, res: any) => {
    res.writeHead(200, { "content-type": "text/event-stream" });
    if ((req.tools ?? []).some((t: any) => t.function?.name === "submit_result")) {
      actor.push(req);
      // 挂住每个 actor 请求（原 run 与修订出的新 run 都保持 live）：就地调并发需要 run 还在飞。
      hungRes.push(res);
      return;
    }
    main.push(req);
    const n = main.length;
    if (n === 1) {
      event(res, call("skill", "Skill", { skill: "dynamic-workflows" }));
      end(res, "tool_calls");
    } else if (n === 2) {
      event(res, call("cw-1", "CreateWorkflow", { script, name: "count-items" }));
      end(res, "tool_calls");
    } else {
      if (n === 3) runId = /dwfrun-[A-Za-z0-9_-]+/.exec(toolResult(req, "cw-1"))?.[0] ?? "missing";
      event(res, { content: "noted" });
      end(res, "stop");
    }
  };
  const env = {
    ZCODE_OFFICIAL_PLUGINS_BASE_DIR: dirname(nodeBundle),
    ZCODE_PLUGIN_HOST_EXEC_PATH: process.execPath,
    ZCODE_PLUGIN_HOST_ENTRYPOINT: nodeBundle,
  };
  let pump: ReturnType<typeof setInterval> | undefined;
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
  try {
    await configureRegistry(f, false);
    const h: Harness = f.start();
    const workspace = { workspacePath: h.workspace, workspaceKey: h.workspace };
    await h.client.request("workspace/updateDynamicWorkflowPolicy", { workspace, enabled: true }, {
      parse: (value: unknown) => value,
    } as any);
    const id = await h.create();
    await h.subscribe(`conversation/${id}`);
    let seen = 0;
    const answered = new Set<string>();
    pump = setInterval(() => {
      for (; seen < h.messages.length; seen++) {
        for (const delta of h.messages[seen]?.params?.frame?.payload?.deltas ?? []) {
          for (const p of delta.patch?.pendingInteractions ?? []) {
            if (p.kind !== "permission" || answered.has(p.interactionId)) continue;
            answered.add(p.interactionId);
            const once = p.payload.options.find((o: Message) => /allow.?once/i.test(o.kind));
            void h.command(
              h.envelope("resolveInteraction", id, {
                interactionId: p.interactionId,
                answer: { optionId: once.optionId },
              }),
            );
          }
        }
      }
    }, 20);

    const scrubText = (text: string) =>
      text
        .replaceAll(JSON.stringify(root).slice(1, -1), "<root>")
        .replaceAll(root, "<root>")
        .replace(/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/g, "<uuid>")
        .replace(/dwf[a-z]*[-_][A-Za-z0-9_-]+/g, "<id>")
        .replace(/Today's date is [^.]*\./g, "Today's date is <date>.");
    const normalize = (value: unknown) => JSON.parse(scrubText(JSON.stringify(value)));
    const projectRow = (row: any) =>
      normalize({
        kind: row.kind,
        origin: row.origin ?? null,
        executionKind: row.executionKind ?? null,
        state: row.state ?? null,
        text: row.text ?? null,
        hasClientId: row.clientId !== undefined,
        hasActiveMs: row.activeMs !== undefined,
        hasHistoryRoundCount: row.historyRoundCount !== undefined,
        hasEndedAt: row.endedAt !== undefined,
        sourceCommandIdPresent: typeof row.sourceCommandId === "string",
        rootSourceCommandIdSameAsSource: row.rootSourceCommandId === row.sourceCommandId,
        workflowLaunch: row.workflowLaunch ?? null,
      });
    const settingsRows = async () =>
      ((await h.rows(id)).rows as any[]).filter((r) => r.origin === "workflowLaunch");

    // 起一个 live 的 run（actor 首请求挂住），再在它身上跑各项设置修订。
    await h.command(h.envelope("sendText", id, { text: "count the items", mode: "yolo" }));
    await h.completed(id);
    assert.ok(await until(() => actor.length >= 1), `${kind}: actor never started`);

    const rawAck = async (payload: Message) =>
      (await h.command(h.envelope("amendWorkflowRunSettings", id, payload))) as any;
    const projectAck = (result: any) => ({
      status: result.status,
      reasonCode: result.reasonCode ?? null,
      message: result.message === undefined ? null : scrubText(String(result.message)),
      result: result.result === undefined ? null : normalize(result.result),
    });

    // 四条零副作用拒绝（旧 run 照旧 live）。
    const unchanged = projectAck(await rawAck({ workId: runId }));
    const notFound = projectAck(await rawAck({ workId: "dwfrun-missing" }));
    const modelUnavailable = projectAck(
      await rawAck({ workId: runId, subagentModel: "personal:fixture/nope" }),
    );

    // 修订出新 run：换子代理模型 → 停下前驱、铸后继（supersededRunId 在场）。
    const supersedeRaw = await rawAck({ workId: runId, subagentModel: "personal:fixture/model-b" });
    const supersede = projectAck(supersedeRaw);
    // 就地调并发 / 被替代前驱都要用**真实**的新 run id（投影版已被 scrub 成 <id>）。
    const newRunId = supersedeRaw.result.runId;

    // 被替代的前驱不再可配置。
    const notConfigurable = projectAck(await rawAck({ workId: runId }));

    // 等修订出的新 run 的 actor 起跑，再就地调并发（run 得在飞，retune 才原地生效）。
    assert.ok(await until(() => actor.length >= 2, 30_000), `${kind}: amended run never started`);

    // 就地调并发：只改 maxConcurrency → 同一个 run、无 supersededRunId。取 1（低于本机天花板
    // `availableParallelism()-2`），否则等于天花板会被读成「没有自己的界」→ unchanged。
    const retune = projectAck(await rawAck({ workId: newRunId, maxConcurrency: 1 }));

    // 设置轮在 ACK 之后落定（主会话空闲时两侧都立即落）。等两轮（supersede + retune）各两行到齐。
    assert.ok(await until(async () => (await settingsRows()).length >= SETTINGS_ROWS, 30_000), `${kind}: settings turns never landed`);
    const rows = (await settingsRows()).map(projectRow);

    clearInterval(pump);
    // 释放挂住的 actor 响应（被 supersede 停下的那个已被客户端断开），宿主才能干净退出。
    for (const res of hungRes) {
      try {
        event(res, { content: "released" });
        end(res, "stop");
      } catch {
        // 已被客户端断开。
      }
    }
    await h.close();

    return {
      unchanged,
      notFound,
      modelUnavailable,
      supersede,
      notConfigurable,
      retune,
      settingsRows: rows,
      schemaErrors: h.schemaErrors,
    };
  } finally {
    clearInterval(pump);
    await f.close();
  }
}

test("Node and Rust amend workflow run settings from the GUI the same way", async () => {
  const node = await observe("node");
  const rust = await observe("rust");
  assert.deepEqual(node.schemaErrors, []);
  // 自检：确认 Node 侧真的走到了修订与就地调并发，避免两侧同样为空也判等。
  assert.equal(node.unchanged.reasonCode, "fault.command.workflowRunSettingsRejected.unchanged");
  assert.equal(node.notFound.reasonCode, "fault.command.workflowRunSettingsRejected.not_found");
  assert.equal(
    node.modelUnavailable.reasonCode,
    "fault.command.workflowRunSettingsRejected.model_unavailable",
  );
  assert.equal(node.supersede.status, "accepted");
  assert.match(String(node.supersede.result?.supersededRunId ?? ""), /^dwfrun-<uuid>$/);
  assert.equal(
    node.notConfigurable.reasonCode,
    "fault.command.workflowRunSettingsRejected.not_configurable",
  );
  if (CAN_RETUNE) {
    assert.equal(node.retune.status, "accepted");
    assert.equal(node.retune.result?.supersededRunId, undefined);
  } else {
    assert.equal(node.retune.reasonCode, "fault.command.workflowRunSettingsRejected.unchanged");
  }
  assert.equal(node.settingsRows.length, SETTINGS_ROWS);
  for (const key of Object.keys(node)) {
    assert.deepEqual((rust as any)[key], (node as any)[key], key);
  }
});
