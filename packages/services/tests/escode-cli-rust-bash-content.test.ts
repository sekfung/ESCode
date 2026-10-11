import assert from "node:assert/strict";
import test from "node:test";
import { resolve } from "node:path";
import { fixture, event, end } from "./escode-cli-rust-fixture.js";
import { configureRegistry } from "./escode-cli-rust-registry-fixture.js";

// docs/specs/rust-bash-model-content.md：Bash 结果进入模型上下文的正文与 Node 逐字一致
// （任务 id 与落盘路径每次不同，按占位比较）。
// 七次 Bash（含 1s 超时与后台任务）加两侧 shell 启动，超过 fixture 默认 8s 的等待。
process.env.ESCODE_TEST_WAIT_MS ??= "30000";
const nodeBundle = resolve("apps/escode-cli/packages/cli/dist/escode.cjs");
const CALLS: Record<string, unknown>[] = [
  { command: "printf '\\n  \\nhello\\n  world  \\n\\n'", description: "success" },
  { command: "echo out; echo err 1>&2; exit 3", description: "provider error" },
  { command: "printf 'a\\n' | grep zzz", description: "no match" },
  { command: "mkdir -p made", description: "silent" },
  // 以 sleep 开头的命令不转后台（TS isBashAutoBackgroundEligible），超时后被终止。
  { command: "sleep 5", timeout: 1000, description: "timeout" },
  // 非 sleep 开头的前台命令超时后转为后台任务（docs/specs/rust-bash-auto-background.md）。
  { command: "echo started; sleep 3", timeout: 1000, description: "auto background" },
  { command: "sleep 1", run_in_background: true, description: "background" },
  { command: `node -e "process.stdout.write('line\\n'.repeat(8000))"`, description: "large" },
  // 图片 stdout（1x1 PNG）：以图片块进入模型；fixture 模型不支持图片，两侧都换成占位文本。
  {
    command:
      "printf 'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg=='",
    description: "image",
  },
  // GitHub 限流提示（进程内 60s 冷却，每个进程只出现一次）。gh 不实际执行（`true ||` 短路）：
  // 之前真实调用 gh，两侧是否联网成功各不相同，stdout 时有时无，差分不稳定。
  {
    command: "true || gh api repos/x/y; echo 'API rate limit exceeded for user'",
    description: "gh",
  },
];

function normalize(content: string) {
  return content
    .replace(/(ID: )[^\s.]+/g, "$1<id>")
    .replace(/((?:saved|written) to: ).+?(?=\.?(?:\n|$| ))/g, "$1<path>");
}

async function observe(kind: "node" | "rust") {
  const seen: string[] = [];
  const respond = (req: any, res: any) => {
    res.writeHead(200, { "content-type": "text/event-stream" });
    const results = req.messages.filter((m: any) => m.role === "tool");
    // 按结果序号记录：后台任务完成后可能多一次模型请求（Windows CI 较慢时出现），重复记录会多出一条。
    if (results.length) seen[results.length - 1] = normalize(String(results.at(-1).content));
    const next = CALLS[results.length];
    if (next) {
      event(res, {
        tool_calls: [
          {
            index: 0,
            id: `c${results.length}`,
            type: "function",
            function: { name: "Bash", arguments: JSON.stringify(next) },
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

test("Bash results reach the model with the same text as Node", async () => {
  const node = await observe("node");
  const rust = await observe("rust");
  assert.equal(node.length, CALLS.length);
  assert.equal(node[0], "hello\n  world");
  assert.match(node[7]!, /^<persisted-output>\nOutput too large \(39\.1KB\)/);
  for (const [index, call] of CALLS.entries()) {
    assert.equal(rust[index], node[index], String(call.description));
  }
});
