import assert from "node:assert/strict";
import test from "node:test";
import { mkdtemp, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import { z } from "zod";
import { fixture, event, end, type Harness } from "./escode-cli-rust-fixture.js";
import { configureRegistry } from "./escode-cli-rust-registry-fixture.js";

// docs/specs/rust-legacy-session-methods.md：App 仍在用的 legacy session/* 请求在 Node 与 Rust 上语义一致。
const nodeBundle = resolve("apps/escode-cli/packages/cli/dist/escode.cjs");
const any = z.any();
const PROVIDER = "personal:fixture";
const B64_MARK = "ATTACH-B64-CONTENT";
const PATH_MARK = "ATTACH-PATH-CONTENT";
type Runtime = "node" | "rust";
const createModes: Partial<Record<Runtime, unknown[]>> = {};

const lastUserText = (req: any) =>
  JSON.stringify(req.messages.filter((m: any) => m.role === "user").at(-1)?.content ?? "");

async function settle(h: Harness, sessionId: string) {
  const started = Date.now();
  while (Date.now() - started < 15_000) {
    const snapshot: any = await h.client.request("session/read", { sessionId }, any);
    if (snapshot.session.status !== "running" && snapshot.messages.length > 0) return snapshot;
    await delay(50);
  }
  throw new Error(`session ${sessionId} did not settle`);
}

const settings = (snapshot: any) => ({
  provider: snapshot.settings.model.current?.providerId,
  model: snapshot.settings.model.current?.modelId,
  thought: snapshot.settings.thoughtLevel.current,
  mode: snapshot.projection?.mode,
});

async function observe(kind: Runtime) {
  const root = await mkdtemp(join(tmpdir(), `escode-legacy-session-${kind}-`));
  const requests: any[] = [];
  const respond = async (req: any, res: any) => {
    requests.push(req);
    if (lastUserText(req).includes("slow")) await delay(1500);
    res.writeHead(200, { "content-type": "text/event-stream" });
    event(res, { content: "ok" });
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
  const out: Record<string, unknown> = {};
  try {
    await configureRegistry(f);
    const h = f.start();
    const workspace = { workspacePath: f.cwd, workspaceKey: f.cwd };
    const create = (params: Record<string, unknown>) =>
      h.client.request("session/create", { workspace, ...params }, any) as Promise<any>;

    // 1. 普通建会话（带模型/档位/模式），以及「无导入却指定 sessionId」的拒绝。
    const draft = await create({
      // App（task facade / session service）建会话时总把档位同时放进 thoughtLevel；
      // 只在 model.options 里带档位时 Node 快照不显示档位，属已知差异（见 spec）。
      model: { providerId: PROVIDER, modelId: "model-b", options: { reasoningLevel: "high" } },
      thoughtLevel: "high",
      mode: "build",
      persistence: "deferred",
    });
    const draftId = draft.session.sessionId;
    out.created = settings(draft);
    await assert.rejects(create({ sessionId: "sess_explicit" }), /imported history/);

    // 2. 配置变更。
    const call = (
      method: "session/setModel" | "session/setThoughtLevel" | "session/setMode",
      params: Record<string, unknown>,
    ) => h.client.request(method, { sessionId: draftId, ...params }, any) as Promise<any>;
    out.setModel = settings(
      await call("session/setModel", {
        model: { providerId: PROVIDER, modelId: "model-a", options: { reasoningLevel: "low" } },
      }),
    );
    out.setThoughtLevel = settings(await call("session/setThoughtLevel", { thoughtLevel: "high" }));
    out.setModeEdit = settings(await call("session/setMode", { mode: "edit" }));
    out.setModePlan = settings(await call("session/setMode", { mode: "plan" }));
    out.setModeBuild = settings(await call("session/setMode", { mode: "build" }));

    // 3. 条件关闭草稿成立。
    out.closeDraft = await h.client.request(
      "session/close",
      { sessionId: draftId, expectedPersistence: "deferred" },
      any,
    );

    // 4. legacy 发送：base64 文本附件 + 本地路径附件，模型请求要带上两份内容。
    const sent = await create({ persistence: "deferred" });
    const sentId = sent.session.sessionId;
    const localPath = join(f.cwd, "note.txt");
    await writeFile(localPath, PATH_MARK);
    const ack: any = await h.client.request(
      "session/send",
      {
        sessionId: sentId,
        inputId: "legacy-input-1",
        content: "read attachments",
        attachments: [
          {
            kind: "file",
            filename: "inline.txt",
            mimeType: "text/plain",
            sizeBytes: B64_MARK.length,
            dataBase64: Buffer.from(B64_MARK).toString("base64"),
          },
          {
            kind: "file",
            filename: "note.txt",
            mimeType: "text/plain",
            sizeBytes: PATH_MARK.length,
            localPath,
          },
        ],
      },
      any,
    );
    out.sendAck = { accepted: ack.accepted, sessionId: ack.sessionId === sentId };
    await settle(h, sentId);
    const body = JSON.stringify(requests.at(-1));
    out.attachmentsReachedModel = [body.includes(B64_MARK), body.includes(PATH_MARK)];

    // 5. 已发送的会话不再是草稿：条件关闭不成立。
    out.closeSent = await h.client.request(
      "session/close",
      { sessionId: sentId, expectedPersistence: "deferred" },
      any,
    );

    // 6. 运行中 legacy 发送被拒（legacy 不排队）。
    await h.client.request("session/send", { sessionId: sentId, content: "slow" }, any);
    await assert.rejects(
      h.client.request("session/send", { sessionId: sentId, content: "again" }, any),
      /already running/,
    );
    await settle(h, sentId);
    out.closeImmediate = await h.client.request("session/close", { sessionId: sentId }, any);

    // 7. 省略 persistence（= immediate）的空会话：Node 首条输入前同样不进会话库，重启后不可恢复。
    const empty = await create({ mode: "yolo" });
    const emptyId = empty.session.sessionId;
    // 已知差异（见 spec）：Node legacy create 的快照不体现 mode 参数（yolo/edit 都读回 build），
    // Rust 按参数生效。单独记录，不进入两侧逐字比较。
    createModes[kind] = [settings(empty).mode, settings(await create({ mode: "edit" })).mode];
    // 已发送的会话重启后可恢复。
    assert.deepEqual(h.schemaErrors, []);
    await h.close();
    const restarted = f.start();
    const resume = (sessionId: string) =>
      restarted.client
        .request("session/resume", { sessionId, workspace }, any)
        .then((r: any) => r.session.sessionId === sessionId)
        .catch(() => false);
    out.resumedAfterRestart = { empty: await resume(emptyId), sent: await resume(sentId) };
    await restarted.close();
    return out;
  } finally {
    await f.close();
  }
}

test("legacy session methods behave the same on Node and Rust", async () => {
  const node = await observe("node");
  const rust = await observe("rust");
  assert.deepEqual(rust, node);
  assert.deepEqual(node.created, {
    provider: PROVIDER,
    model: "model-b",
    thought: "high",
    mode: "build",
  });
  assert.deepEqual(node.attachmentsReachedModel, [true, true]);
  assert.deepEqual(node.closeDraft, { closed: true });
  assert.deepEqual(node.closeSent, { closed: false });
  assert.deepEqual(node.resumedAfterRestart, { empty: false, sent: true });
  assert.deepEqual(createModes.rust, ["yolo", "edit"]);
  assert.deepEqual(createModes.node, ["build", "build"]);
});
