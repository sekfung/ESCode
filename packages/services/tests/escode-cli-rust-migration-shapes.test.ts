import assert from "node:assert/strict";
import test from "node:test";
import { copyFile, mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fixture } from "./escode-cli-rust-fixture.js";
import { copyLegacySeed } from "./escode-cli-rust-legacy-seed.js";
import { canonical, nodeOracle } from "./escode-cli-rust-golden.js";

// docs/specs/rust-import-lifecycle.md「真实数据演练」：真实数据暴露的三种存储形态已冻结为仓内种子，
// Node runtime 的观察录成 golden；正常跑只启动 Rust 并与 golden 逐行比对（不依赖用户数据）。
const nodeBundle = resolve("apps/escode-cli/packages/cli/dist/escode.cjs");
const session = "shape-session";

async function observe(kind: "node" | "rust", source: string) {
  const root = await mkdtemp(join(tmpdir(), `escode-shapes-${kind}-`));
  await copyFile(source, join(root, "ts.sqlite"));
  const f =
    kind === "node"
      ? await fixture({
          root,
          command: process.execPath,
          args: ({ cwd }) => [nodeBundle, "app-server", "--stdio", "--cwd", cwd],
        })
      : await fixture({ root, legacy: true });
  try {
    const h = f.start("shape-workspace");
    await h.subscribe(`conversation/${session}`);
    const rows = (await h.rows(session)).rows as Record<string, any>[];
    const observation = {
      kinds: rows.map((r) => r.kind),
      userTexts: rows.filter((r) => r.kind === "userInput").map((r) => r.text),
      markers: rows
        .filter((r) => r.kind === "timelineMarker")
        .map((r) => [r.marker.type, r.marker.fromModel ?? null, r.marker.toModel]),
      schemaErrors: h.schemaErrors,
    };
    await h.close();
    return observation;
  } finally {
    await f.close();
  }
}

test("Rust imports rewind branches, model changes and empty reasoning exactly as Node shows them", async () => {
  const dir = await mkdtemp(join(tmpdir(), "escode-shapes-seed-"));
  const source = join(dir, "ts.sqlite");
  await copyLegacySeed("migration-shapes.shapes", source, {
    workspace: "shape-workspace",
    root: dir,
  });
  // Node 侧观察只用于录制；正常跑直接加载 golden，不再启动 Node runtime。
  const node = await nodeOracle("observe", () => observe("node", source));
  const rust = canonical(await observe("rust", source));
  if (process.env.ESCODE_SHAPES_DUMP) console.log("DUMP", JSON.stringify({ node, rust }));
  assert.deepEqual(rust, node);
  // 自检：用例确实覆盖了三种形态（避免两侧同时为空而误判一致）。
  assert.deepEqual(node.userTexts, ["first question", "after rewind"]);
  assert(!node.kinds.includes("reasoning"), "empty reasoning must not render");
});
