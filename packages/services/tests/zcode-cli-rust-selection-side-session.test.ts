import assert from "node:assert/strict";
import test from "node:test";
import { mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import { z } from "zod";
import { fixture, event, end, type Harness } from "./zcode-cli-rust-fixture.js";
import { configureRegistry } from "./zcode-cli-rust-registry-fixture.js";

// docs/specs/rust-v4-command-gaps.md：createSelectionSideSession（框选副屏）在 Node 与 Rust 上的
// ACK、子会话可见行、模型请求的会话部分、列表可见性与副屏受限命令一致。
const nodeBundle = resolve("apps/zcode-cli/packages/cli/dist/zcode.cjs");
const any = z.any();
type Runtime = "node" | "rust";

const text = (content: unknown) =>
  typeof content === "string" ? content : JSON.stringify(content);

/**
 * 会话部分（去掉 system）；临时工作区路径归一化。另去掉两条与副屏无关的提醒：
 * - Skill 列表：夹具没给 Rust 传官方插件目录（桌面端由 Host 传入），只有 Node 列出 browser-use 技能；
 * - shell 环境变更提醒（`The Bash tool shell is …`）：Rust 尚未实现恢复期 shell 提醒，见 spec「已知差异」。
 */
const UNRELATED_REMINDERS = ["The following skills are available", "The Bash tool shell is"];
function conversation(messages: any[], cwd: string) {
  return messages
    .filter((m) => m.role !== "system")
    .map((m) => ({ role: m.role, content: text(m.content).replaceAll(cwd, "<cwd>") }))
    .filter(
      (m) => !UNRELATED_REMINDERS.some((r) => m.content.startsWith(`<system-reminder>\n${r}`)),
    );
}

async function settled(h: Harness, sessionId: string) {
  const started = Date.now();
  while (Date.now() - started < 15_000) {
    const page = await h.rows(sessionId);
    const rows = page.rows as any[];
    const header = rows.findLast((r) => r.kind === "turnHeader");
    if (header && header.state !== "running") return rows;
    await delay(50);
  }
  throw new Error(`session ${sessionId} did not settle`);
}

async function observe(kind: Runtime) {
  const root = await mkdtemp(join(tmpdir(), `zcode-side-chat-${kind}-`));
  const requests: any[] = [];
  const respond = (req: any, res: any) => {
    requests.push(req);
    res.writeHead(200, { "content-type": "text/event-stream" });
    event(res, { content: `answer ${requests.length}` });
    end(res, "stop");
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
    const parent = await h.create();
    await h.subscribe(`conversation/${parent}`);
    await h.command(h.envelope("sendText", parent, { text: "parent question", mode: "yolo" }));
    await h.completed(parent);
    const parentRowCount = (await h.rows(parent)).rows.length;

    // 带首条输入的副屏：子会话起跑，父会话不变。
    const ack: any = await h.command(
      h.envelope("createSelectionSideSession", parent, {
        firstInput: { text: "explain the selection" },
      }),
    );
    const child = ack.result.sessionId;
    const childRows = await settled(h, child);
    const request = requests.at(-1);
    const childRead: any = await h.client.request("session/read", { sessionId: child }, any);
    const list: any = await h.client.request(
      "session/list",
      { workspace: { workspacePath: f.cwd, workspaceKey: f.cwd } },
      any,
    );
    // 副屏内受限命令。
    const goal = await h.command(
      h.envelope("sendGoalCommand", child, { text: "do something", mode: "yolo" }),
    );
    // 不带首条输入的副屏。
    const bare: any = await h.command(h.envelope("createSelectionSideSession", parent, {}));
    const out = {
      ack: {
        status: ack.status,
        type: ack.result.type,
        hasInput: Boolean(ack.result.input),
        delivery: ack.result.input?.delivery ?? null,
        childIdPrefix: String(child).slice(0, 5),
      },
      childRowKinds: childRows.map((r) => r.kind),
      // toThought：Node 副屏 child 投影的 config.thought 为空串，Rust 用实际档位（已知差异，单独断言）。
      childMarker: childRows
        .filter((r) => r.kind === "timelineMarker")
        .map((r) => ({
          type: r.marker.type,
          toProvider: r.marker.toProvider,
          toModel: r.marker.toModel,
          lane: r.lane,
          initial: String(r.entityId).startsWith("model-initial:"),
        })),
      childUserText: childRows.find((r) => r.kind === "userInput")?.text ?? null,
      parentRowsUnchanged: (await h.rows(parent)).rows.length === parentRowCount,
      requestConversation: conversation(request.messages, f.cwd),
      childKind: childRead.session.sessionKind,
      childTitle: childRead.session.title,
      childListed: (list.sessions as any[]).some((s) => s.sessionId === child),
      goal: { status: goal.status, reasonCode: goal.reasonCode ?? null },
      bare: { status: bare.status, type: bare.result?.type, hasInput: Boolean(bare.result?.input) },
    };
    assert.deepEqual(h.schemaErrors, []);
    await h.close();
    return out;
  } finally {
    await f.close();
  }
}

test("selection side session behaves the same on Node and Rust", async () => {
  const node = await observe("node");
  const rust = await observe("rust");
  assert.deepEqual(rust, node);
  assert.equal(node.ack.type, "createSelectionSideSession");
  assert.equal(node.childListed, false);
  assert.equal(node.goal.reasonCode, "guard.selectionSideChatRestrictedCommand");
});
