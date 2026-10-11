import assert from "node:assert/strict";
import test from "node:test";
import { dirname, join, resolve } from "node:path";
import { mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { z } from "zod";
import { fixture, event, end } from "./escode-cli-rust-fixture.js";
import { configureRegistry } from "./escode-cli-rust-registry-fixture.js";

// docs/specs/rust-dynamic-workflow.md：随 CLI 内置的技能包（bundled-skills / dynamic-workflows）。
// 动态工作流开启的会话里模型能看到 `dynamic-workflows` 技能，关闭的会话里看不到；它永远不进 `$` 引用面板。
// 两侧都从 Node CLI 产物旁的 packages/bundled-skills 发现（Rust 经 Host 给的基准目录）。
process.env.ESCODE_TEST_WAIT_MS ??= "30000";
const nodeBundle = resolve("apps/escode-cli/packages/cli/dist/escode.cjs");
const SKILL = "dynamic-workflows";

async function observe(kind: "node" | "rust") {
  const root = await mkdtemp(join(tmpdir(), `escode-bundled-skills-${kind}-`));
  const requests: any[] = [];
  const respond = (req: any, res: any) => {
    requests.push(req);
    res.writeHead(200, { "content-type": "text/event-stream" });
    event(res, { content: "done" });
    end(res, "stop");
  };
  const env = {
    ESCODE_OFFICIAL_PLUGINS_BASE_DIR: dirname(nodeBundle),
    ESCODE_PLUGIN_HOST_EXEC_PATH: process.execPath,
    ESCODE_PLUGIN_HOST_ENTRYPOINT: nodeBundle,
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
  try {
    await configureRegistry(f, false);
    const h = f.start();
    const workspace = { workspacePath: h.workspace, workspaceKey: h.workspace };
    const policy = (enabled: boolean) =>
      h.client.request(
        "workspace/updateDynamicWorkflowPolicy",
        { workspace, enabled },
        { parse: (value: unknown) => value } as any,
      );
    const turn = async () => {
      const before = requests.length;
      const id = await h.create();
      await h.subscribe(`conversation/${id}`);
      await h.command(h.envelope("sendText", id, { text: "hello", mode: "yolo" }));
      await h.completed(id);
      return JSON.stringify(requests.slice(before)).includes(SKILL);
    };
    await policy(true);
    const seenWhenEnabled = await turn();
    await policy(false);
    const seenWhenDisabled = await turn();
    await policy(true);
    const catalog = (await h.client.request(
      "skills/referenceCatalog" as any,
      { workspace },
      z.any(),
    )) as any;
    const inReferenceCatalog = catalog.skills.some((s: any) => s.name === SKILL);
    await h.close();
    return { seenWhenEnabled, seenWhenDisabled, inReferenceCatalog };
  } finally {
    await f.close();
  }
}

test("Node and Rust expose the bundled dynamic-workflows skill the same way", async () => {
  const node = await observe("node");
  const rust = await observe("rust");
  assert.deepEqual(node, {
    seenWhenEnabled: true,
    seenWhenDisabled: false,
    inReferenceCatalog: false,
  });
  assert.deepEqual(rust, node);
});
