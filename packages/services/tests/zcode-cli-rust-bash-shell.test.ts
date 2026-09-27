import assert from "node:assert/strict";
import test from "node:test";
import { writeFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import { fixture, event, end } from "./zcode-cli-rust-fixture.js";
import { configureRegistry } from "./zcode-cli-rust-registry-fixture.js";

// docs/specs/rust-bash-shell-snapshot.md：用户 ~/.bashrc 的函数与 alias 经 shell 初始化快照可用；
// 出错行号、离开工作区时的 cwd 重置提示与 Node 一致（临时目录名按占位比较）。
// 有 ~/.bashrc 时 Node 每次 Bash 在 Windows 上约 5–8s（快照 source 与 Git Bash 启动），六次调用超过 30s。
process.env.ZCODE_TEST_WAIT_MS ??= "120000";
const nodeBundle = resolve("apps/zcode-cli/packages/cli/dist/zcode.cjs");
const CALLS: Record<string, unknown>[] = [
  { command: "greet_from_rc world" },
  { command: "rc_alias" },
  { command: "cd missing-dir" },
  { command: "mkdir -p sub && cd sub && pwd -P | sed 's#.*/##'" },
  { command: "cd .. && cd .. && echo moved" },
  { command: "cd sub; false" },
];

async function observe(kind: "node" | "rust") {
  const seen: string[] = [];
  const respond = (req: any, res: any) => {
    res.writeHead(200, { "content-type": "text/event-stream" });
    const results = req.messages.filter((m: any) => m.role === "tool");
    if (results.length) {
      seen.push(
        String(results.at(-1).content)
          .replace(/zcode-cli-rust-test-[A-Za-z0-9]+/g, "<tmp>")
          .replaceAll("\\", "/"),
      );
    }
    const next = CALLS[results.length];
    if (next) {
      event(res, {
        tool_calls: [
          {
            index: 0,
            id: `c${results.length}`,
            type: "function",
            function: { name: "Bash", arguments: JSON.stringify({ ...next, description: "x" }) },
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
    await configureRegistry(f, false);
    // fixture 的 HOME 为 f.root（见 zcode-cli-rust-fixture.ts）。
    await writeFile(
      join(f.root, ".bashrc"),
      "greet_from_rc() { echo \"hello $1\"; }\nalias rc_alias='echo from-alias'\n",
    );
    const h = f.start();
    const id = await h.create();
    await h.subscribe(`conversation/${id}`);
    await h.command(h.envelope("sendText", id, { text: "run" }));
    await h.completed(id);
    await h.close();
    return seen;
  } finally {
    await f.close();
  }
}

// Windows 上 TS 自身创建快照约 9.7s（Git Bash login profile 定义大量函数，逐个 base64 导出），贴着 10s 超时：
// 两侧各自随机成功或回落 login shell，结果不确定（规格已记录）。规则逐字相同，在 Linux/macOS 上比较。
const windows = process.platform === "win32";
test(
  "Bash sees the user's shell init and reports cwd and errors the same way as Node",
  { skip: windows && "Git Bash snapshot creation sits at TS's 10s timeout on Windows" },
  async () => {
    const node = await observe("node");
    const rust = await observe("rust");
    assert.equal(node.length, CALLS.length);
    assert.equal(node[0], "hello world");
    assert.equal(node[1], "from-alias");
    for (const [index, call] of CALLS.entries()) {
      assert.equal(rust[index], node[index], String(call.command));
    }
  },
);
