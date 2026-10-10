import assert from "node:assert/strict";
import test from "node:test";
import { mkdtemp, rm } from "node:fs/promises";
import { existsSync } from "node:fs";
import { join, resolve } from "node:path";
import { tmpdir } from "node:os";
import { DatabaseSync } from "node:sqlite";
import { event, end, fixture } from "./escode-cli-rust-fixture.js";
import { configureRegistry } from "./escode-cli-rust-registry-fixture.js";

// docs/specs/rust-shell-selection.md「已知差异」：曾记「TS 把 shell 选择落库为 session entry，
// 冷恢复沿用创建时的 shell；Rust 只做进程内缓存、冷恢复重新问 Host」。
// 实测（本用例）：**App stdio 链路上 TS 也没有写该 entry**——phase 1 结束后 Node 的
// `session_entry` 只有 `runtime/model_selection` 与 `runtime/execution_state`，没有
// `bash_shell_selection`；两侧冷恢复都按当前设置重新解析，改设置对既有会话立即生效。
// 差分口径：同一 workspace、同一会话，先以 Git Bash 跑一次，改成 CMD 后重启进程续跑，
// 比较两次 Bash 的实际 shell（`ver` 只在 cmd 下输出 Windows 版本）。
const nodeBundle = resolve("apps/escode-cli/packages/cli/dist/escode.cjs");
const gitBash = "C:\\Program Files\\Git\\bin\\bash.exe";
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
function text(response: any, content: string) {
  event(response, { content });
  end(response, "stop");
}
function respond(request: any, response: any) {
  response.writeHead(200, { "content-type": "text/event-stream" });
  const last = request.messages.at(-1);
  if (last.role === "user") return call(response, "bash-1");
  return text(response, "done");
}

async function observe(kind: Runtime) {
  const root = await mkdtemp(join(tmpdir(), `escode-shell-resume-${kind}-`));
  const start = async (shell: Record<string, unknown> | undefined) => {
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
    await configureRegistry(f);
    const h = f.start();
    if (shell) h.integratedTerminalShell = shell;
    return { f, h };
  };
  try {
    // 阶段 1：用户在设置里选 Git Bash（本机自动探测在沙箱里不一定是 Git Bash，显式指定更稳）。
    const first = await start({
      mode: "shell",
      dialect: "git-bash",
      id: "git-bash",
      label: "Git Bash",
      path: gitBash,
    });
    const sid = await first.h.create();
    await first.h.subscribe(`conversation/${sid}`);
    await first.h.command(first.h.envelope("sendText", sid, { text: "first" }));
    await first.h.completed(sid);
    const firstTool = first.f.requests
      .at(-1)
      ?.messages.filter((m: any) => m.role === "tool")
      .at(-1)?.content;
    await first.h.close();
    await first.f.close();
    // Node 侧自检：App stdio 链路不落 shell 快照 entry（Rust 用自己的库，不参与该断言）。
    const entryTypes =
      kind === "node"
        ? (() => {
            const db = new DatabaseSync(join(root, "ts.sqlite"));
            const rows = db.prepare("SELECT DISTINCT type FROM session_entry").all() as {
              type: string;
            }[];
            db.close();
            return rows.map((r) => r.type).sort();
          })()
        : [];

    // 阶段 2：用户在设置里改成 CMD，冷恢复同一会话再跑一次。
    const second = await start({
      mode: "shell",
      dialect: "cmd",
      id: "cmd",
      label: "CMD",
      path: "cmd.exe",
    });
    await second.h.subscribe(`conversation/${sid}`);
    await second.h.command(second.h.envelope("sendText", sid, { text: "second" }));
    await second.h.completed(sid);
    const secondTool = second.f.requests
      .at(-1)
      ?.messages.filter((m: any) => m.role === "tool")
      .at(-1)?.content;
    await second.h.close();
    await second.f.close();
    return { shell: { firstTool, secondTool }, entryTypes };
  } finally {
    await rm(root, { recursive: true, force: true });
  }
}

test(
  "changing the shell setting applies to an existing session on both runtimes",
  { skip: process.platform !== "win32" || !existsSync(gitBash) },
  async () => {
    const node = await observe("node");
    const rust = await observe("rust");
    assert.deepEqual(rust.shell, node.shell);
    // 自检：阶段 1 走的是 Git Bash（cmd 才有 Windows 版本行），阶段 2 改成 CMD 后二者都换过来。
    assert.doesNotMatch(String(node.shell.firstTool), /Microsoft Windows/);
    assert.match(String(node.shell.secondTool), /Microsoft Windows/);
    // 钉住原因：App stdio 链路没有 shell 快照 entry，冷恢复只能按当前设置解析。
    assert.deepEqual(node.entryTypes, [
      "runtime/execution_state",
      "runtime/model_selection",
    ]);
  },
);
