import assert from "node:assert/strict";
import test from "node:test";
import { mkdtemp, rm } from "node:fs/promises";
import { existsSync } from "node:fs";
import { join, resolve } from "node:path";
import { tmpdir } from "node:os";
import { event, end, fixture } from "./zcode-cli-rust-fixture.js";
import { configureRegistry } from "./zcode-cli-rust-registry-fixture.js";

// docs/specs/rust-shell-selection.md：会话内的 shell 在会话开始时固定一次。
// 场景：同一进程、同一会话，先按 Host 的 Git Bash 跑一条 Bash，再把设置改成 CMD，
// 同会话再跑一条——两侧都**不**改用新 shell（Node 由 config.bashShellSelection 决定，
// Rust 由 (session, user-execution) 进程内缓存决定）。跨进程续跑会重新解析，见
// zcode-cli-rust-shell-resume.test.ts。
const nodeBundle = resolve("apps/zcode-cli/packages/cli/dist/zcode.cjs");
const gitBash = "C:\\Program Files\\Git\\bin\\bash.exe";
const cmdShell = { mode: "shell", dialect: "cmd", id: "cmd", label: "CMD", path: "cmd.exe" };
type Runtime = "node" | "rust";

function call(response: any, id: string) {
  event(response, {
    tool_calls: [
      {
        index: 0,
        id,
        type: "function",
        function: { name: "Bash", arguments: JSON.stringify({ command: "ver" }) },
      },
    ],
  });
  end(response, "tool_calls");
}

async function observe(kind: Runtime) {
  const root = await mkdtemp(join(tmpdir(), `zcode-shell-switch-${kind}-`));
  const respond = (request: any, response: any) => {
    response.writeHead(200, { "content-type": "text/event-stream" });
    if (request.messages.at(-1).role === "user") return call(response, `bash-${Date.now()}`);
    event(response, { content: "done" });
    end(response, "stop");
  };
  const f =
    kind === "node"
      ? await fixture({
          root,
          command: process.execPath,
          args: ({ cwd }) => [nodeBundle, "app-server", "--stdio", "--cwd", cwd],
          registry: true,
          mode: "yolo",
          respond,
        })
      : await fixture({ root, registry: true, mode: "yolo", respond });
  try {
    await configureRegistry(f);
    const h = f.start();
    h.integratedTerminalShell = {
      mode: "shell",
      dialect: "git-bash",
      id: "git-bash",
      label: "Git Bash",
      path: gitBash,
    };
    const sid = await h.create();
    await h.subscribe(`conversation/${sid}`);
    const toolResult = (index: number) =>
      f.requests
        .slice(0, index + 1)
        .at(-1)
        ?.messages.filter((m: any) => m.role === "tool")
        .at(-1)?.content;
    await h.command(h.envelope("sendText", sid, { text: "first" }));
    await h.completed(sid);
    const first = toolResult(f.requests.length - 1);
    // 同一进程内改设置，再跑一条。
    h.integratedTerminalShell = cmdShell;
    await h.command(h.envelope("sendText", sid, { text: "second" }));
    await h.completed(sid);
    const second = toolResult(f.requests.length - 1);
    await h.close();
    return { first, second };
  } finally {
    await f.close();
    await rm(root, { recursive: true, force: true });
  }
}

test(
  "a mid-session shell setting change does not affect the running session on either runtime",
  { skip: process.platform !== "win32" || !existsSync(gitBash) },
  async () => {
    const node = await observe("node");
    const rust = await observe("rust");
    assert.deepEqual(rust, node);
    assert.doesNotMatch(String(node.first), /Microsoft Windows/);
    assert.doesNotMatch(String(node.second), /Microsoft Windows/);
  },
);
