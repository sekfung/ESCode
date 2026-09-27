import assert from "node:assert/strict";
import test from "node:test";
import { writeFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import { fixture, event, end } from "./zcode-cli-rust-fixture.js";
import { configureRegistry } from "./zcode-cli-rust-registry-fixture.js";

// docs/specs/rust-bash-model-content.md：Bash 读文件命令回填读取状态（之后可直接 Edit/Write），
// 格式化/修复类命令改写已读文件时提示重新 Read；与 Node 逐字一致。
process.env.ZCODE_TEST_WAIT_MS ??= "30000";
const nodeBundle = resolve("apps/zcode-cli/packages/cli/dist/zcode.cjs");
const CALLS: [string, unknown][] = [
  // cat 读过的文件记为已读，随后 Edit 无需 Read（TS 回填 readFileState）。
  ["Bash", { command: "cat existing.txt", description: "cat" }],
  ["Edit", { file_path: "existing.txt", old_string: "original", new_string: "edited" }],
  // head 回填的是片段但非 partial view：Write 也可直接进行。
  ["Bash", { command: "head -n 1 empty.txt", description: "head" }],
  ["Write", { file_path: "empty.txt", content: "filled\n" }],
  // 读过的文件被格式化/修复类命令改写：提示重新 Read。
  ["Read", { file_path: "existing.txt" }],
  // 可移植的改写（BSD sed 没有 --in-place）；命令里带 `--fix` 标记即算格式化/修复类命令。
  ["Bash", { command: "echo fixed > existing.txt && echo applied --fix", description: "fix" }],
  ["Bash", { command: "echo unrelated --fix", description: "marker without change" }],
  // 重复 Read：同一范围且文件未变时返回未变提示；换范围则给出正文。
  ["Read", { file_path: "existing.txt", offset: 1, limit: 1 }],
  ["Read", { file_path: "existing.txt", offset: 1, limit: 1 }],
  ["Read", { file_path: "existing.txt", offset: 2 }],
];

async function observe(kind: "node" | "rust") {
  const seen: string[] = [];
  let parameters: Record<string, string> = {};
  let cwd = "";
  const respond = (req: any, res: any) => {
    res.writeHead(200, { "content-type": "text/event-stream" });
    const results = req.messages.filter((m: any) => m.role === "tool");
    if (!results.length) {
      parameters = Object.fromEntries(
        req.tools.map((t: any) => [t.function.name, JSON.stringify(t.function.parameters)]),
      );
    }
    if (results.length) seen.push(String(results.at(-1).content).split(cwd).join("<cwd>"));
    const next = CALLS[results.length];
    if (next) {
      event(res, {
        tool_calls: [
          {
            index: 0,
            id: `c${results.length}`,
            type: "function",
            function: { name: next[0], arguments: JSON.stringify(next[1]) },
          },
        ],
      });
      end(res, "tool_calls");
    } else {
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
          mode: "yolo",
        })
      : await fixture({ registry: true, respond, mode: "yolo" });
  try {
    cwd = f.cwd;
    await configureRegistry(f, false);
    await writeFile(join(f.cwd, "existing.txt"), "original\n");
    await writeFile(join(f.cwd, "empty.txt"), "");
    const h = f.start();
    const id = await h.create();
    await h.subscribe(`conversation/${id}`);
    await h.command(h.envelope("sendText", id, { text: "go" }));
    await h.completed(id);
    await h.close();
    return { seen, parameters };
  } finally {
    await f.close();
  }
}

test("Bash reads and rewrites update read state the same way as Node", async () => {
  const node = await observe("node");
  const rust = await observe("rust");
  assert.equal(node.seen.length, CALLS.length);
  // 先比较两侧，再确认用例确实覆盖到回填与过期提示（CI 上 Node 的行为因环境而异时，先看到两侧差异）。
  for (const [index, [name, args]] of CALLS.entries()) {
    assert.equal(rust.seen[index], node.seen[index], `${name} ${JSON.stringify(args)}`);
  }
  assert.match(node.seen[1]!, /has been updated successfully/);
  assert.match(node.seen[5]!, /you've previously read: existing\.txt/);
});
