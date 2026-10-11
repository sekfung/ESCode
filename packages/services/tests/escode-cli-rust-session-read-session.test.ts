import assert from "node:assert/strict";
import test from "node:test";
import { dirname, join, resolve } from "node:path";
import { mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { escodeSessionStateSnapshotSchema } from "@escode/shared";
import { fixture, event, end } from "./escode-cli-rust-fixture.js";
import { configureRegistry } from "./escode-cli-rust-registry-fixture.js";

// docs/specs/rust-v4-command-gaps.md「session/read 的 session 投影」：`session` 与 `settings` 两个
// 投影块逐字段与 Node 的 `mapSessionInfo` / `mapSessionSettings` 对齐——traceId 的存在性、无目标时的
// 显式 `target: null`、`session.model` 只带 providerId/modelId（档位归 settings）、未命名会话不发
// `titleSource`（新会话为空、有首条输入后为 first_input）。冷恢复的会话投影由 Rust 侧冷读用例
// （`escode-cli-rust-session-read.test.ts`）覆盖。
process.env.ESCODE_TEST_WAIT_MS ??= "60000";
type Runtime = "node" | "rust";
const nodeBundle = resolve("apps/escode-cli/packages/cli/dist/escode.cjs");
const UUIDS = /[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/g;

async function observe(kind: Runtime) {
  const root = await mkdtemp(join(tmpdir(), `escode-session-read-session-${kind}-`));
  const env = {
    ESCODE_OFFICIAL_PLUGINS_BASE_DIR: dirname(nodeBundle),
    ESCODE_PLUGIN_HOST_EXEC_PATH: process.execPath,
    ESCODE_PLUGIN_HOST_ENTRYPOINT: nodeBundle,
  };
  const respond = (_req: any, res: any) => {
    event(res, { content: "noted" });
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
          mode: "yolo",
          env,
        })
      : await fixture({ root, registry: true, respond, mode: "yolo", env });
  const scrub = (value: string) =>
    value
      .replaceAll(JSON.stringify(root).slice(1, -1), "<root>")
      .replaceAll(root, "<root>")
      .replace(UUIDS, "<uuid>");
  const at = (value: unknown) => (value === undefined ? "<missing>" : typeof value);
  const project = (snapshot: any) =>
    JSON.parse(
      scrub(
        JSON.stringify({
          session: {
            sessionKind: snapshot.session?.sessionKind ?? null,
            title: snapshot.session?.title ?? null,
            // 键是否存在与取值分开看：`titleSource` 缺席与 `null` 是两种不同的表达。
            hasTitleSource: snapshot.session?.titleSource !== undefined,
            titleSource: snapshot.session?.titleSource ?? null,
            mode: snapshot.session?.mode ?? null,
            status: snapshot.session?.status ?? null,
            createdAtType: at(snapshot.session?.createdAt),
            updatedAtType: at(snapshot.session?.updatedAt),
            traceIdType: at(snapshot.session?.traceId),
            hasTarget: snapshot.session?.target !== undefined,
            target: snapshot.session?.target ?? null,
            model: snapshot.session?.model ?? null,
            modelKeys: snapshot.session?.model ? Object.keys(snapshot.session.model).sort() : null,
            workspaceKeys: Object.keys(snapshot.session?.workspace ?? {}).sort(),
            parentSessionId: snapshot.session?.parentSessionId ?? null,
          },
          settings: {
            modelCurrent: snapshot.settings?.model?.current ?? null,
            thoughtCurrent: snapshot.settings?.thoughtLevel?.current ?? null,
          },
        }),
      ),
    ) as Record<string, any>;
  const read = (client: any, sessionId: string) =>
    client.request("session/read", { sessionId }, escodeSessionStateSnapshotSchema);
  try {
    await configureRegistry(f, false);
    const h = f.start();
    const id = await h.create();
    await h.subscribe(`conversation/${id}`);
    // 空草稿还没落库，重启后就查不到了；先跑一轮真实回合成会话事实。
    const fresh = project(await read(h.client, id));
    await h.command(h.envelope("sendText", id, { text: "hello" }));
    await h.completed(id);
    const warm = project(await read(h.client, id));
    const schemaErrors = h.schemaErrors;
    await h.close();
    if (schemaErrors.length) throw new Error(`${kind}: ${JSON.stringify(schemaErrors)}`);
    return { fresh, warm };
  } finally {
    await f.close();
  }
}

test("Node and Rust project the session/read session block the same way", async () => {
  const node = await observe("node");
  // 自检：确认 Node 侧真的带上这几个字段、新会话真的省掉 titleSource，避免两侧同为缺失也判等。
  assert.equal(node.fresh.session.traceIdType, "string");
  assert.equal(node.fresh.session.hasTarget, true);
  assert.equal(node.fresh.session.target, null);
  assert.deepEqual(node.fresh.session.modelKeys, ["modelId", "providerId"]);
  assert.equal(node.fresh.session.hasTitleSource, false);
  assert.equal(node.warm.session.titleSource, "first_input");
  const rust = await observe("rust");
  assert.deepEqual(rust, node);
});
