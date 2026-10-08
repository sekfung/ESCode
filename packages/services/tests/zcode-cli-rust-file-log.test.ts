import assert from "node:assert/strict";
import test from "node:test";
import { mkdtemp, readdir, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fixture } from "./zcode-cli-rust-fixture.js";

// docs/specs/rust-file-log.md：两个 runtime 写同一日志目录（桌面导出日志收集的 ~/.zcode/cli/log），
// 各写各的日文件；Rust 条目与 Node toSerializableEntry 同形。
const nodeBundle = resolve("apps/zcode-cli/packages/cli/dist/zcode.cjs");

async function logs(kind: "node" | "rust") {
  const root = await mkdtemp(join(tmpdir(), `zcode-file-log-${kind}-`));
  const f =
    kind === "node"
      ? await fixture({
          root,
          command: process.execPath,
          args: ({ cwd }) => [nodeBundle, "app-server", "--stdio", "--cwd", cwd],
        })
      : await fixture({ root });
  try {
    const h = f.start();
    await h.create();
    await h.close();
  } finally {
    await f.close();
  }
  const dir = join(root, ".zcode", "cli", "log");
  const files = (await readdir(dir)).filter((name) => name.endsWith(".jsonl")).sort();
  const entries = (
    await Promise.all(files.map((name) => readFile(join(dir, name), "utf8")))
  )
    .flatMap((text) => text.split("\n").filter(Boolean))
    .map((line) => JSON.parse(line));
  await rm(root, { recursive: true, force: true });
  return { files, entries };
}

test("Rust writes its own daily JSONL log next to Node's, in Node's entry shape", async () => {
  const node = await logs("node");
  const rust = await logs("rust");
  assert.ok(node.files.some((name) => /^zcode-\d{4}-\d{2}-\d{2}\.jsonl$/.test(name)));
  assert.deepEqual(
    rust.files.map((name) => name.replace(/\d{4}-\d{2}-\d{2}/, "<date>")),
    ["zcode-rust-<date>.jsonl"],
  );
  const started = rust.entries.find((entry) => entry.event === "runtime.started");
  assert.ok(started, "runtime.started missing");
  // Node 条目的必备字段 Rust 都有；时间为 ISO UTC。
  const nodeKeys = new Set(Object.keys(node.entries[0]));
  for (const key of ["timestamp", "level", "module", "message"]) {
    assert.ok(nodeKeys.has(key), `node ${key}`);
    assert.ok(key in started, `rust ${key}`);
  }
  assert.match(started.timestamp, /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$/);
  assert.equal(started.level, "info");
});
