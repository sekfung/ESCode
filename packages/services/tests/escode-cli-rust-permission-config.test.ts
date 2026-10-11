import assert from "node:assert/strict";
import test from "node:test";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import { event, end, fixture } from "./escode-cli-rust-fixture.js";
import { configureRegistry } from "./escode-cli-rust-registry-fixture.js";

// docs/specs/rust-permission-modes.md「工具能力表」：`permission.allowedTools` / `disallowedTools`
// 来自 `~/.escode/cli/config.json`（与 --allowed-tools/--disallowed-tools 同源）。Node 用它做
// 精确名字的硬禁用/直通；Rust 之前完全没读这段配置，遇到被禁用的工具会照常弹确认。
const nodeBundle = resolve("apps/escode-cli/packages/cli/dist/escode.cjs");
type Runtime = "node" | "rust";

function writeCall(res: any, id: string) {
  event(res, {
    tool_calls: [
      {
        index: 0,
        id,
        type: "function",
        function: {
          name: "Write",
          arguments: JSON.stringify({ file_path: "out.txt", content: "written" }),
        },
      },
    ],
  });
  end(res, "tool_calls");
}

async function observe(
  kind: Runtime,
  permission: Record<string, unknown>,
  mode: "build" | "yolo",
) {
  let step = 0;
  const respond = (_req: any, res: any) => {
    res.writeHead(200, { "content-type": "text/event-stream" });
    if (step++ === 0) writeCall(res, "w1");
    else {
      event(res, { content: "done" });
      end(res, "stop");
    }
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
    JSON.stringify({ permission }),
  );
  try {
    await configureRegistry(f);
    const h = f.start();
    const id = await h.create();
    await h.subscribe(`conversation/${id}`);
    assert.equal(
      (await h.command(h.envelope("sendText", id, { text: "write", mode }))).status,
      "accepted",
    );
    await h.completed(id);
    const rows = (await h.rows(id)).rows as any[];
    const row = rows.find((r) => r.kind === "toolCall" && r.toolName === "Write");
    const observation = {
      status: row?.status,
      // 模型看到的工具结果正文（拒绝文案或写入结果）。
      toolResult: f.requests[1]?.messages.filter((m: any) => m.role === "tool").at(-1)?.content,
      file: await readFile(join(f.cwd, "out.txt"), "utf8").catch(() => undefined),
      // 配置命中时不得弹确认。
      prompts: h.messages.filter((m: any) =>
        m.params?.frame?.payload?.deltas?.some((d: any) =>
          d.patch?.pendingInteractions?.some((p: any) => p.kind === "permission"),
        ),
      ).length,
      schemaErrors: h.schemaErrors,
    };
    await h.close();
    return observation;
  } finally {
    await f.close();
  }
}

test("Node and Rust both hard-deny a tool listed in permission.disallowedTools", async () => {
  const node = await observe("node", { disallowedTools: ["Write"] }, "build");
  const rust = await observe("rust", { disallowedTools: ["Write"] }, "build");
  assert.deepEqual(rust, node);
  assert.equal(node.status, "cancelled");
  assert.equal(node.prompts, 0, "硬禁用不得退化成弹窗");
  assert.equal(node.file, undefined);
  assert.equal(node.toolResult, "Tool Write is explicitly disallowed");
});

test("Node and Rust both pre-approve a tool listed in permission.allowedTools", async () => {
  const node = await observe("node", { allowedTools: ["Write"] }, "build");
  const rust = await observe("rust", { allowedTools: ["Write"] }, "build");
  assert.deepEqual(rust, node);
  assert.equal(node.status, "success");
  assert.equal(node.prompts, 0);
  assert.equal(node.file, "written");
});

test("yolo still bypasses permission.disallowedTools on both runtimes", async () => {
  const node = await observe("node", { disallowedTools: ["Write"] }, "yolo");
  const rust = await observe("rust", { disallowedTools: ["Write"] }, "yolo");
  assert.deepEqual(rust, node);
  assert.equal(node.status, "success");
  assert.equal(node.file, "written");
});
