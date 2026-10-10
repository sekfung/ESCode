import assert from "node:assert/strict";
import test from "node:test";
import { mkdir, writeFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import { event, end, fixture } from "./escode-cli-rust-fixture.js";
import { configureRegistry } from "./escode-cli-rust-registry-fixture.js";

// docs/specs/rust-subagents.md：`features.subagent=false` 时 TS 不注入 SubagentPort，
// Agent/SendMessage 既不进模型可见工具面，直接调用也报 ConfigurationError。
// 这里比对两侧的模型可见工具名清单（顺序也一致），默认值必须仍然是「都在」。
const nodeBundle = resolve("apps/escode-cli/packages/cli/dist/escode.cjs");
type Runtime = "node" | "rust";

async function toolNames(kind: Runtime, subagent: boolean) {
  const respond = (_req: any, res: any) => {
    res.writeHead(200, { "content-type": "text/event-stream" });
    event(res, { content: "done" });
    end(res, "stop");
  };
  const f =
    kind === "node"
      ? await fixture({
          command: process.execPath,
          args: ({ cwd }) => [nodeBundle, "app-server", "--stdio", "--cwd", cwd],
          registry: true,
          respond,
        })
      : await fixture({ registry: true, respond });
  // 两端都读 `~/.escode/cli/config.json`（fixture 的 HOME=root），进程启动前写入即可。
  await mkdir(join(f.root, ".escode", "cli"), { recursive: true });
  await writeFile(
    join(f.root, ".escode", "cli", "config.json"),
    JSON.stringify({ features: { subagent } }),
  );
  try {
    await configureRegistry(f);
    const h = f.start();
    const id = await h.create();
    await h.subscribe(`conversation/${id}`);
    await h.command(h.envelope("sendText", id, { text: "hi", mode: "yolo" }));
    await h.completed(id);
    // node_repl 只在 Node 的 bundle 中随官方插件出现（不属于内置工具面），差分只比内置工具。
    const names = (f.requests[0]?.tools ?? [])
      .map((t: any) => String(t.function.name))
      .filter((name: string) => !name.startsWith("mcp__"));
    assert.deepEqual(h.schemaErrors, []);
    await h.close();
    return names;
  } finally {
    await f.close();
  }
}

test("features.subagent=false hides Agent/SendMessage from the tool surface on both runtimes", async () => {
  const node = await toolNames("node", false);
  const rust = await toolNames("rust", false);
  assert.deepEqual(rust, node);
  assert.ok(!node.includes("Agent"), JSON.stringify(node));
  assert.ok(!node.includes("SendMessage"), JSON.stringify(node));
});

test("the default tool surface still offers Agent/SendMessage on both runtimes", async () => {
  const node = await toolNames("node", true);
  const rust = await toolNames("rust", true);
  assert.deepEqual(rust, node);
  assert.ok(node.includes("Agent"));
  assert.ok(node.includes("SendMessage"));
});
