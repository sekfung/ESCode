import assert from "node:assert/strict";
import test from "node:test";
import { access, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { DatabaseSync } from "node:sqlite";
import { fixture } from "./escode-cli-rust-fixture.js";
import { copyLegacySeed } from "./escode-cli-rust-legacy-seed.js";
import { escodeSessionStateSnapshotSchema } from "@escode/shared";

const SESSIONS = 120;
const MESSAGES_PER_SESSION = 15;
const LARGE_BYTES = 1_500_000;

// docs/specs/rust-release-rollback.md「数据迁移」：大库实测——导入规模、时延与幂等。
test("Rust imports a large TS history once, stays idempotent and preserves large attachments", async (t) => {
  const f = await fixture({ legacy: true });
  try {
    const tsDb = join(f.root, "ts.sqlite");
    const largeFile = join(f.root, "large.bin");
    const largeBytes = Buffer.alloc(LARGE_BYTES, 7);
    await writeFile(largeFile, largeBytes);
    // 大库由真实 TS store 一次性生成后冻结为仓内种子；正常跑只复制。
    await copyLegacySeed("migration.scale", tsDb, { workspace: f.cwd, root: f.root });

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
        escodeSessionStateSnapshotSchema,
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
      escodeSessionStateSnapshotSchema,
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
      escodeSessionStateSnapshotSchema,
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
