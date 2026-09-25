import assert from "node:assert/strict";
import test from "node:test";
import { createHash } from "node:crypto";
import { dirname, join, relative, resolve } from "node:path";
import { mkdtemp, readdir, readFile, stat } from "node:fs/promises";
import { tmpdir } from "node:os";
import { fixture, event, end } from "./zcode-cli-rust-fixture.js";
import { configureRegistry } from "./zcode-cli-rust-registry-fixture.js";

// docs/specs/rust-official-plugin-seed.md：Node 与 Rust 共享同一官方插件缓存。
// 任一 runtime 先 seed，另一个启动时必须认出缓存为当前版本、不改写任何文件，且发现相同的插件技能。
const nodeBundle = resolve("apps/zcode-cli/packages/cli/dist/zcode.cjs");
const text = (m: any) => (typeof m.content === "string" ? m.content : JSON.stringify(m.content));

async function snapshot(dir: string) {
  const files: Record<string, { sha: string; mtimeMs: number }> = {};
  const walk = async (current: string) => {
    for (const entry of await readdir(current, { withFileTypes: true })) {
      const path = join(current, entry.name);
      if (entry.isDirectory()) await walk(path);
      else if (entry.isFile())
        files[relative(dir, path).replaceAll("\\", "/")] = {
          sha: createHash("sha256")
            .update(await readFile(path))
            .digest("hex"),
          mtimeMs: (await stat(path)).mtimeMs,
        };
    }
  };
  await walk(dir);
  return files;
}

async function run(kind: "node" | "rust", root: string) {
  let main: any;
  const respond = (req: any, res: any) => {
    if (req.stream === false || req.stream === undefined) {
      res.writeHead(200, { "content-type": "application/json" });
      res.end(
        JSON.stringify({
          id: "t",
          object: "chat.completion",
          choices: [
            { index: 0, message: { role: "assistant", content: "Title" }, finish_reason: "stop" },
          ],
          usage: { prompt_tokens: 1, completion_tokens: 1, total_tokens: 2 },
        }),
      );
      return;
    }
    if (!text(req.messages[0] ?? {}).startsWith("Generate a concise title")) main ??= req;
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
      : await fixture({
          root,
          registry: true,
          respond,
          env: {
            // 开发树中 Rust 可执行文件不在插件源旁：与 Node 入口使用同一候选目录与插件宿主。
            ZCODE_OFFICIAL_PLUGINS_BASE_DIR: dirname(nodeBundle),
            ZCODE_PLUGIN_HOST_EXEC_PATH: process.execPath,
            ZCODE_PLUGIN_HOST_ENTRYPOINT: nodeBundle,
          },
        });
  try {
    await configureRegistry(f);
    const h = f.start();
    const id = await h.create();
    await h.subscribe(`conversation/${id}`);
    await h.command(h.envelope("sendText", id, { text: "hi", mode: "yolo" }));
    await h.completed(id);
    await h.close();
  } finally {
    await f.close();
  }
  const messages = (main?.messages ?? []) as any[];
  return {
    skills: messages.map(text).find((t) => t.includes("skills are available")),
    system: messages
      .filter((m) => m.role === "system")
      .map(text)
      .join(""),
  };
}

for (const [first, second] of [
  ["node", "rust"],
  ["rust", "node"],
] as const)
  test(`${first} seeds the official plugin cache and ${second} reuses it without rewriting`, async () => {
    const root = await mkdtemp(join(tmpdir(), `zcode-seed-${first}-`));
    const storage = join(root, ".zcode", "cli", "plugins");
    const a = await run(first, root);
    const seeded = await snapshot(storage);
    // 自检：确实 seed 出官方插件（marker 与 marketplace）。
    assert.ok(Object.keys(seeded).some((p) => p.endsWith(".zcode-plugin-seed.json")));
    assert.ok(seeded["marketplaces/zcode-plugins-official/bundled-marketplace.json"]);
    const b = await run(second, root);
    assert.deepEqual(await snapshot(storage), seeded);
    assert.ok(a.skills?.includes("browser-use:control-browser"));
    assert.equal(b.skills, a.skills);
    assert.equal(
      b.system.replace(/zcode-seed-\w+-\w+/g, "ROOT"),
      a.system.replace(/zcode-seed-\w+-\w+/g, "ROOT"),
    );
  });
