import assert from "node:assert/strict";
import test from "node:test";
import { mkdtemp, rm } from "node:fs/promises";
import { existsSync } from "node:fs";
import { join, resolve } from "node:path";
import { tmpdir } from "node:os";
import { event, end, fixture } from "./zcode-cli-rust-fixture.js";
import { configureRegistry } from "./zcode-cli-rust-registry-fixture.js";

// docs/specs/rust-shell-resume-notice.md：恢复会话后的 `The Bash tool shell is …` 提醒。
// 同一会话跑三段（phase 1 → 重启 → phase 2 → 重启 → phase 3），比较 phase 2 / 3 的模型请求会话部分：
// 提醒的有无、正文、位置（本段输入之前），以及不落库（phase 3 里 phase 2 的提醒消失、换成新的一条）。
const nodeBundle = resolve("apps/zcode-cli/packages/cli/dist/zcode.cjs");
const gitBash = "C:\\Program Files\\Git\\bin\\bash.exe";
const GIT_BASH = { mode: "shell", dialect: "git-bash", id: "git-bash", label: "Git Bash", path: gitBash };
const CMD = { mode: "shell", dialect: "cmd", id: "cmd", label: "CMD", path: "cmd.exe" };
type Runtime = "node" | "rust";
type Shell = Record<string, unknown> | undefined;

const text = (content: unknown) =>
  typeof content === "string" ? content : JSON.stringify(content);
/** 夹具没给 Rust 传官方插件目录，只有 Node 列出 browser-use 技能（同 fork-child 用例的取舍）。 */
const UNRELATED = "The following skills are available";

function respond(_request: any, response: any) {
  response.writeHead(200, { "content-type": "text/event-stream" });
  event(response, { content: "done" });
  end(response, "stop");
}

async function observe(kind: Runtime, before: Shell, after: Shell) {
  const root = await mkdtemp(join(tmpdir(), `zcode-shell-notice-${kind}-`));
  const conversation = (request: any) =>
    (request?.messages ?? [])
      .filter((m: any) => m.role !== "system" && !text(m.content).includes(UNRELATED))
      .map((m: any) => ({
        role: m.role,
        content: text(m.content).replace(/Today's date is [^.]*\./g, "Today's date is <date>."),
      }));
  const phase = async (shell: Shell, input: string, sid?: string) => {
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
      if (shell) h.integratedTerminalShell = shell;
      const id = sid ?? (await h.create());
      await h.subscribe(`conversation/${id}`);
      const mark = h.messages.length;
      await h.command(h.envelope("sendText", id, { text: input }));
      await h.completed(id, mark);
      const messages = conversation(f.requests.at(-1));
      await h.close();
      return { id, messages };
    } finally {
      await f.close();
    }
  };
  try {
    const first = await phase(before, "first");
    const second = await phase(after, "second", first.id);
    const third = await phase(after, "third", first.id);
    return { second: second.messages, third: third.messages };
  } finally {
    await rm(root, { recursive: true, force: true });
  }
}

const notice = (name: string) => ({
  role: "user",
  content: `<system-reminder>\nThe Bash tool shell is ${name}.\n</system-reminder>`,
});
const tail = (messages: any[]) => messages.slice(-2);

test(
  "resumed sessions get the same shell reminder on both runtimes",
  { skip: process.platform !== "win32" || !existsSync(gitBash) },
  async () => {
    const cases: [string, Shell, Shell, string | undefined][] = [
      // 用户把 Git Bash 改成 CMD：提示词里记的是 Git Bash，提醒新的显示名。
      ["git-bash -> cmd", GIT_BASH, CMD, "CMD"],
      // 默认自动探测到 Git Bash：每次恢复都提醒。
      ["auto -> auto", undefined, undefined, "Git Bash"],
      // 显式 CMD 不变：不提醒。
      ["cmd -> cmd", CMD, CMD, undefined],
    ];
    for (const [name, before, after, expected] of cases) {
      const node = await observe("node", before, after);
      const rust = await observe("rust", before, after);
      // 自检 Node 基准：提醒紧挨在本段输入之前；下一次恢复时旧提醒不在历史里（不落库）。
      if (expected) {
        assert.deepEqual(tail(node.second), [notice(expected), { role: "user", content: "second" }], name);
        assert.deepEqual(tail(node.third), [notice(expected), { role: "user", content: "third" }], name);
        assert.equal(
          node.third.filter((m: any) => m.content.includes("The Bash tool shell is")).length,
          1,
          name,
        );
      } else {
        assert.ok(
          !node.third.some((m: any) => m.content.includes("The Bash tool shell is")),
          name,
        );
      }
      assert.deepEqual(rust, node, name);
    }
  },
);
