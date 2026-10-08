import assert from "node:assert/strict";
import test from "node:test";
import { mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { z } from "zod";
import { fixture, event, end } from "./zcode-cli-rust-fixture.js";
import { configureRegistry } from "./zcode-cli-rust-registry-fixture.js";

// docs/specs/rust-v4-command-gaps.md「forkAssistant 的 child 身份与边界」：
// fork 出的 child 在 Node 与 Rust 上身份（id/taskType/title/titleSource）、列表可见性、
// 边界行、child 首次模型请求的会话部分，以及 session/read 的 session 投影一致。
const nodeBundle = resolve("apps/zcode-cli/packages/cli/dist/zcode.cjs");
const any = z.any();
type Runtime = "node" | "rust";

const text = (content: unknown) =>
  typeof content === "string" ? content : JSON.stringify(content);

/**
 * 会话部分（去掉 system）；工作区路径归一化。另去掉一条本次不涉及的提醒：
 * - Skill 列表：夹具没给 Rust 传官方插件目录（桌面端由 Host 传入），只有 Node 列出 browser-use 技能；
 * shell 环境变更提醒（`The Bash tool shell is …`）不再过滤：两侧都在 child 首轮注入，逐字比较
 * （docs/specs/rust-shell-resume-notice.md）。
 */
const UNRELATED_REMINDERS = ["The following skills are available"];

async function observe(kind: Runtime) {
  const root = await mkdtemp(join(tmpdir(), `zcode-fork-child-${kind}-`));
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
    await h.command(h.envelope("sendText", parent, { text: "first question", mode: "yolo" }));
    await h.completed(parent);
    const rows = await h.rows(parent);
    const stable = (rows.rows as any[]).find((r) => r.kind === "assistantText")!;
    const fork: any = await h.command({
      ...h.envelope("forkAssistant", parent, {
        target: { rowId: stable.rowId, entityId: stable.entityId },
      }),
      baseRevision: rows.atRevision,
      baseLogEpoch: rows.atLogEpoch,
    });
    const child = fork.result?.sessionId as string;
    const parentRead: any = await h.client.request("session/read", { sessionId: parent }, any);
    await h.subscribe(`conversation/${child}`);
    const childRows: any = await h.rows(child);
    const childRead: any = await h.client.request("session/read", { sessionId: child }, any);
    const list: any = await h.client.request(
      "session/list",
      { workspace: { workspacePath: f.cwd, workspaceKey: f.cwd } },
      any,
    );
    // 再发一条输入，取 child 自己的模型请求，验证 fork 提醒确实进了 provider transcript。
    await h.command(h.envelope("sendText", child, { text: "child question", mode: "yolo" }));
    await h.completed(child);
    const childRequest = requests.at(-1);

    const mapped = (value: string) =>
      value
        .replaceAll(parent, "<parent>")
        .replaceAll(child, "<child>")
        .replaceAll(stable.entityId, "<targetMessage>")
        .replaceAll(f.cwd, "<cwd>");
    const forkRow = (childRows.rows as any[]).find((r) => r.kind === "timelineMarker");
    const listed = (list.sessions as any[]).find((s) => s.sessionId === child);
    return {
      ack: { status: fork.status, reasonCode: fork.reasonCode ?? null, type: fork.result?.type },
      childIdPrefix: child.slice(0, 5),
      // TS `buildAtomicForkNotice`：entityId = `fork:{parentSessionId}:{targetMessageId}`。
      childRefersToTargetEntity: forkRow?.entityId === `fork:${parent}:${stable.entityId}`,
      forkRow: {
        rowId: forkRow?.rowId,
        kind: forkRow?.kind,
        lane: forkRow?.lane,
        visibility: forkRow?.visibility,
        marker: { ...forkRow?.marker, parentSessionId: mapped(String(forkRow?.marker?.parentSessionId)) },
      },
      // createdAtSeq（Node 按父会话 seq 续号、Rust 从 1 起）与 turnId（Node 新建、Rust 复用被选行）
      // 是已知取值差异，不参与比较；turnHeader 之外的行序与内容必须一致。
      // row / entity id 本身无法跨进程比较，只保留 rowId（一行一轮的夹具里两侧一致）与内容；
      // fork 行的 entityId 含义由 childRefersToTargetEntity 单独断言。
      childRows: (childRows.rows as any[]).map((r) => ({
        kind: r.kind,
        rowId: r.rowId,
        lane: r.lane ?? null,
        markerType: r.marker?.type ?? null,
        markerParentRowId: r.marker?.parentRowId ?? null,
        text: r.text ?? null,
        state: r.state ?? null,
      })),
      childRead: {
        sessionKind: childRead.session?.sessionKind,
        title: childRead.session?.title,
        titleSource: childRead.session?.titleSource,
        parentSessionIdMatches: childRead.session?.parentSessionId === parent,
        // fork child 与父会话共享 traceId（TS `buildForkedSessionInput` 取 runtime root trace）。
        traceIdIsString: typeof childRead.session?.traceId === "string",
        traceIdSharedWithParent: childRead.session?.traceId === parentRead.session?.traceId,
        target: childRead.session?.target ?? null,
        // TS `mapSessionInfo` 的 session.model 来自 `optionalModelSelectionFromString(getModel())`，
        // 只按 `/` 切分，不含 options；Rust 曾多带 options.reasoningLevel。
        model: childRead.session?.model ?? null,
        status: childRead.session?.status,
      },
      // mode 见 spec「已知差异」：Node 用父会话创建时持久化的 permission，Rust 只有一个实时 mode。
      listed: listed
        ? {
            sessionKind: listed.sessionKind,
            title: listed.title,
            parentSessionIdMatches: listed.parentSessionId === parent,
          }
        : null,
      childRequest: (childRequest?.messages ?? [])
        .filter((m: any) => m.role !== "system")
        .map((m: any) => ({ role: m.role, content: mapped(text(m.content)) }))
        .filter(
          (m: any) => !UNRELATED_REMINDERS.some((r) => m.content.startsWith(`<system-reminder>\n${r}`)),
        ),
    };
  } finally {
    await f.close();
  }
}

test("forked session child keeps Node identity, fork boundary and provider transcript on both runtimes", async () => {
  const node = await observe("node");
  const rust = await observe("rust");
  assert.deepEqual(rust, node);
  assert.equal(node.childIdPrefix, "sess_");
  assert.deepEqual(node.ack, { status: "accepted", reasonCode: null, type: "forkAssistant" });
  assert.deepEqual(node.forkRow.marker, { type: "forkNotice", parentSessionId: "<parent>", parentRowId: 0 });
  assert.equal(node.forkRow.kind, "timelineMarker");
  assert.equal(node.forkRow.lane, "turnTailBoundary");
  assert.equal(node.childRefersToTargetEntity, true);
  assert.equal(node.childRead.sessionKind, "fork");
  assert.equal(node.childRead.title, "Fork of first question");
  assert.equal(node.childRead.titleSource, "generated");
  assert.equal(node.childRead.target, null);
  assert.deepEqual(node.childRead.model, { providerId: "personal:fixture", modelId: "model-a" });
  assert.equal(node.childRead.traceIdSharedWithParent, true);
  assert.equal(node.listed?.sessionKind, "fork");
  // fork 提醒以 system-reminder 外壳注入，正文逐字包含 parent / target 边界。
  const notice = node.childRequest.find((m: any) => m.content.includes("This session was forked"));
  assert.ok(notice, "fork notice reaches the provider transcript");
  assert.match(notice.content, /parentSessionId: <parent>/);
  assert.match(notice.content, /No workspace checkpoint was restored for this fork\./);
  assert.deepEqual(node.childRequest.at(-1), { role: "user", content: "child question" });
});
