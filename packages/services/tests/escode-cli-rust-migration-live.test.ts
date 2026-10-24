import assert from "node:assert/strict";
import test from "node:test";
import { copyFile, mkdir, mkdtemp, rm } from "node:fs/promises";
import { join, resolve } from "node:path";
import { tmpdir } from "node:os";
import { fixture } from "./escode-cli-rust-fixture.js";
import { configureRegistry } from "./escode-cli-rust-registry-fixture.js";
import { escodeSessionStateSnapshotSchema } from "@escode/shared";
import { DatabaseSync } from "node:sqlite";
import { nodeOracle } from "./escode-cli-rust-golden.js";

const nodeBundle = resolve("apps/escode-cli/packages/cli/dist/escode.cjs");
const goldenDir = resolve("packages/services/tests/fixtures/cli-golden");
// Node 真实写出的 TS 数据冻结为仓内 seed：正常跑只复制这份 DB 给 Rust 导入，不再启动 Node。
const seedDb = join(goldenDir, "migration-live.legacy-ts.sqlite");
// 录制时的临时 root 不能进仓：DB 里的绝对路径（含 JSON 转义形式）先改写成占位符，回放时再换成本次 root。
const ROOT_PLACEHOLDER = "__ESCODE_SEED_ROOT_RAW__";
const ROOT_PLACEHOLDER_JSON = "__ESCODE_SEED_ROOT_JSON__";
// 该用例在录制时串行启动真实 Node runtime 再导入，Windows runner 冷启动可能超过默认 8s。
process.env.ESCODE_TEST_WAIT_MS ??= "30000";

/** 把所有 TEXT 列里的路径从 `from` 换成 `to`（含 JSON 字符串里的转义形式）。 */
function rewritePaths(
  db: DatabaseSync,
  from: string,
  to: string,
  jsonFrom: string,
  jsonTo: string,
) {
  const tables = db
    .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'")
    .all() as { name: string }[];
  for (const { name } of tables) {
    const columns = db.prepare(`PRAGMA table_info(${JSON.stringify(name)})`).all() as {
      name: string;
      type: string;
    }[];
    for (const column of columns) {
      // 只改文本列：数值列不会被路径命中，但 replace() 会把它们临时转成文本。
      if (column.type && !/text/i.test(column.type)) continue;
      for (const [needle, replacement] of [
        [from, to],
        [jsonFrom, jsonTo],
      ] as const) {
        if (!needle) continue;
        db.prepare(
          `UPDATE ${JSON.stringify(name)} SET ${JSON.stringify(column.name)} = replace(${JSON.stringify(column.name)}, ?, ?) WHERE ${JSON.stringify(column.name)} LIKE ?`,
        ).run(needle, replacement, `%${needle}%`);
      }
    }
  }
}

/** 录制阶段：真实 Node runtime 跑一轮会话，并把落盘的 TS 数据库复制成 seed。 */
async function observeNode() {
  // 自己持有 root：Node 阶段结束后数据要冻结/交给 Rust 导入（fixture 只清理自己创建的目录）。
  const root = await mkdtemp(join(tmpdir(), "escode-ts-live-"));
  const nodeFixture = await fixture({
    root,
    command: process.execPath,
    args: ({ cwd }) => [nodeBundle, "app-server", "--stdio", "--cwd", cwd],
    registry: true,
  });
  let sessionId = "";
  let schemaErrors: string[] = [];
  try {
    await configureRegistry(nodeFixture);
    const h = nodeFixture.start();
    sessionId = await h.create();
    await h.subscribe(`conversation/${sessionId}`);
    // 显式 yolo：默认 build 下 Write 会等用户确认，这里要的是「Node 真实写出的工具行」。
    await h.command(h.envelope("sendText", sessionId, { text: "write", mode: "yolo" }));
    await h.completed(sessionId);
    schemaErrors = h.schemaErrors;
    await h.close();
  } finally {
    await nodeFixture.close();
  }

  // TS 侧确实落库了内容（否则后面的导入断言没有意义），并 checkpoint 后把 DB 冻结成 seed。
  const ts = new DatabaseSync(join(root, "ts.sqlite"));
  let tsMessages = 0;
  try {
    ts.exec("PRAGMA wal_checkpoint(TRUNCATE)");
    tsMessages = (
      ts.prepare("SELECT COUNT(*) AS n FROM message WHERE session_id=?").get(sessionId) as {
        n: number;
      }
    ).n;
    rewritePaths(
      ts,
      root,
      ROOT_PLACEHOLDER,
      JSON.stringify(root).slice(1, -1),
      ROOT_PLACEHOLDER_JSON,
    );
  } finally {
    ts.close();
  }
  assert(tsMessages >= 2, `Node runtime wrote ${tsMessages} messages`);
  await mkdir(goldenDir, { recursive: true });
  await copyFile(join(root, "ts.sqlite"), seedDb);
  await rm(root, { recursive: true, force: true });
  return { sessionId, tsMessages, schemaErrors };
}

// docs/specs/rust-release-rollback.md「数据迁移」：用真实 Node runtime 写出的 TS 数据做导入验收，
// 而不是手写 fixture——这是「真实用户数据」之外能拿到的最接近的口径。正常跑使用冻结的 seed DB。
test("Rust imports a session written by the real Node runtime and preserves its content", async () => {
  const node = await nodeOracle("live-import", observeNode);
  // 1) Node 侧观察（golden）：会话、消息数量与协议 schema 都正常。
  assert(node.tsMessages >= 2, `Node runtime wrote ${node.tsMessages} messages`);

  // 2) Rust runtime 导入冻结的 TS 数据，内容必须逐项对上。
  const root = await mkdtemp(join(tmpdir(), "escode-ts-golden-"));
  await copyFile(seedDb, join(root, "ts.sqlite"));
  // seed 里的占位路径换成本次 root：导入按 workspace（cwd）筛选会话。
  const seed = new DatabaseSync(join(root, "ts.sqlite"));
  try {
    // 先替换 workspace 整段：seed 录制在 Linux，占位符后是 "/workspace"；
    // Windows 回放必须换成 join(root,"workspace") 的反斜杠形式，否则 workspace 匹配失败。
    const workspace = join(root, "workspace");
    rewritePaths(
      seed,
      `${ROOT_PLACEHOLDER}/workspace`,
      workspace,
      `${ROOT_PLACEHOLDER_JSON}/workspace`,
      JSON.stringify(workspace).slice(1, -1),
    );
    rewritePaths(
      seed,
      ROOT_PLACEHOLDER,
      root,
      ROOT_PLACEHOLDER_JSON,
      JSON.stringify(root).slice(1, -1),
    );
  } finally {
    seed.close();
  }
  const rustFixture = await fixture({ root, legacy: true });
  let rustHarness: ReturnType<typeof rustFixture.start> | undefined;
  try {
    const h = rustFixture.start();
    rustHarness = h;
    await h.subscribe(`conversation/${node.sessionId}`);
    const snapshot = await h.client.request(
      "session/read",
      { sessionId: node.sessionId },
      escodeSessionStateSnapshotSchema,
    );
    assert.equal(snapshot.session.title, "write");
    // 行级事实：用户输入、工具调用、助手回复都要在，且与 Node 写的一致。
    const rows = (await h.rows(node.sessionId)).rows;
    const kinds = rows.map((r) => r.kind);
    assert(kinds.includes("turnHeader"), `imported rows missing turn header: ${kinds.join(",")}`);
    assert(
      rows.some((r) => r.kind === "userInput" && (r as { text?: string }).text === "write"),
      "imported rows lost the user input written by the Node runtime",
    );
    assert(
      rows.some((r) => r.kind === "toolCall" && (r as { toolName?: string }).toolName === "Write"),
      "imported rows lost the Write tool call from the Node runtime",
    );
    assert(
      rows.some((r) => r.kind === "assistantText"),
      "imported rows lost the assistant reply from the Node runtime",
    );
    await h.close();
    assert.deepEqual(rustHarness?.schemaErrors, []);
    assert.deepEqual(node.schemaErrors, []);
  } finally {
    await rustFixture.close();
    await rm(root, { recursive: true, force: true });
  }
});
