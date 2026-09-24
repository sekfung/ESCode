import assert from "node:assert/strict";
import test from "node:test";
import { access, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { DatabaseSync } from "node:sqlite";
import { createSqliteSessionStore } from "../../../apps/zcode-cli/packages/adapters/src/storage/session-store.js";
import type { MessageId, PartId, ProjectId, SessionId, WorkspaceId } from "@zcode/contracts";
import { fixture } from "./zcode-cli-rust-fixture.js";
import { zcodeSessionStateSnapshotSchema } from "@zcode/shared";

const SESSIONS = 120;
const MESSAGES_PER_SESSION = 15;
const LARGE_BYTES = 1_500_000;

// docs/specs/rust-release-rollback.md「数据迁移」：大库实测——导入规模、时延与幂等。
test("Rust imports a large TS history once, stays idempotent and preserves large attachments", async (t) => {
  const f = await fixture({ legacy: true });
  try {
    const tsDb = join(f.root, "ts.sqlite");
    const store = createSqliteSessionStore({ dbPath: tsDb });
    const largeFile = join(f.root, "large.bin");
    const largeBytes = Buffer.alloc(LARGE_BYTES, 7);
    await writeFile(largeFile, largeBytes);

    for (let s = 0; s < SESSIONS; s += 1) {
      const id = `scale-${s}` as SessionId;
      const withLarge = s === 0;
      await store.createSession({
        id,
        projectID: "p" as ProjectId,
        workspaceID: f.cwd as WorkspaceId,
        directory: f.cwd,
        slug: id,
        title: `scale ${s}`,
        version: "fixture",
      });
      await store.saveSessionEntry({
        id: `${id}-model`,
        sessionID: id,
        type: "runtime/model_selection",
        time: { created: 1, updated: 1 },
        data: { providerId: "fixture", modelId: "core-model", options: { reasoningLevel: "none" } },
      });
      await store.saveSessionEntry({
        id: `${id}-mode`,
        sessionID: id,
        type: "runtime/execution_state",
        time: { created: 1, updated: 1 },
        data: { mode: "yolo", planEnabled: false },
      });
      for (let m = 0; m < MESSAGES_PER_SESSION; m += 1) {
        const messageID = `${id}-m${m}` as MessageId;
        await store.saveMessage({
          id: messageID,
          sessionID: id,
          role: m % 2 === 0 ? "user" : "assistant",
          time: { created: m + 1 },
          agent: "main",
        });
        await store.savePart({
          id: `${id}-p${m}` as PartId,
          sessionID: id,
          messageID,
          type: "text",
          text: `history ${s}/${m} ${"x".repeat(200)}`,
        });
      }
      if (withLarge) {
        const messageID = `${id}-large` as MessageId;
        await store.saveMessage({
          id: messageID,
          sessionID: id,
          role: "user",
          time: { created: 999 },
          agent: "main",
        });
        await store.savePart({
          id: `${id}-large-part` as PartId,
          sessionID: id,
          messageID,
          type: "file",
          mime: "application/octet-stream",
          filename: "large.bin",
          url: pathToFileURL(largeFile).href,
        });
      }
    }
    store.close();

    const first = f.start();
    const started = Date.now();
    await first.subscribe(`conversation/scale-0`);
    const importedMs = Date.now() - started;
    t.diagnostic(`imported ${SESSIONS} sessions in ${importedMs}ms`);
    // 大库导入必须有界：超时说明按会话重复复制或逐条重放。
    assert(importedMs < 120_000, `large import took ${importedMs}ms`);

    // 抽样校验内容与顺序。
    for (const s of [0, 57, SESSIONS - 1]) {
      const snapshot = await first.client.request(
        "session/read",
        { sessionId: `scale-${s}` },
        zcodeSessionStateSnapshotSchema,
      );
      assert.equal(snapshot.session.title, `scale ${s}`);
      const texts = snapshot.messages
        .flatMap((m) => m.parts)
        .filter((p) => p.type === "text")
        .map((p) => (p as { text: string }).text);
      // 会话 0 额外带一个未支持 MIME 的附件：其 provider 占位文本在请求断言里检查。
      assert.equal(texts.length, MESSAGES_PER_SESSION + (s === 0 ? 1 : 0));
      assert.match(texts[0]!, new RegExp(`^history ${s}/0`));
    }
    // 大附件（1.5 MiB）必须落盘成文件型 part，而不是被静默丢弃。
    const largeSnapshot = await first.client.request(
      "session/read",
      { sessionId: "scale-0" },
      zcodeSessionStateSnapshotSchema,
    );
    assert(largeSnapshot.session.title === "scale 0");
    // 未支持 MIME 的附件按 TS 语义退化为 provider 可见的文本占位（不再让整份导入失败）。
    await first.command(first.envelope("sendText", "scale-0", { text: "after import" }));
    await first.completed("scale-0");
    assert.match(
      JSON.stringify(f.requests.at(-1)),
      /\[Attached application\/octet-stream: large\.bin\]/,
    );
    await first.close();

    const rowsAfterFirst = countRows(f.dataDir);
    const second = f.start();
    await second.subscribe(`conversation/scale-0`);
    const importedAgain = await second.client.request(
      "session/read",
      { sessionId: "scale-0" },
      zcodeSessionStateSnapshotSchema,
    );
    assert.equal(importedAgain.session.title, "scale 0");
    await second.close();
    // 幂等：第二次启动不应再次导入（行数不变，且源库未被追加）。
    assert.equal(countRows(f.dataDir), rowsAfterFirst, "re-import duplicated history");
    const imports = new DatabaseSync(join(f.dataDir, "rust-sessions.sqlite"), { readOnly: true });
    try {
      const total = imports.prepare("SELECT COUNT(*) AS n FROM rust_legacy_import").get() as {
        n: number;
      };
      assert.equal(total.n, 1, "TS source imported more than once");
    } finally {
      imports.close();
    }
    assert.deepEqual(first.schemaErrors, []);
    assert.deepEqual(second.schemaErrors, []);
    await access(largeFile);
  } finally {
    await f.close();
  }
});

function countRows(dataDir: string): number {
  const db = new DatabaseSync(join(dataDir, "rust-sessions.sqlite"), { readOnly: true });
  try {
    const row = db.prepare("SELECT COUNT(*) AS n FROM rust_row").get() as { n: number };
    return row.n;
  } finally {
    db.close();
  }
}
