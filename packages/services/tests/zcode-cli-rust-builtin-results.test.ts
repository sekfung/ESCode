import assert from "node:assert/strict";
import test from "node:test";
import { writeFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import { fixture, event, end } from "./zcode-cli-rust-fixture.js";
import { configureRegistry } from "./zcode-cli-rust-registry-fixture.js";

// docs/specs/rust-tool-input-validation.md（内置工具）与 rust-file-tool-results.md：内置工具的入参校验、
// Read/Write/Edit 的结果与错误文案，以及发给模型的内置 schema 属性顺序，与 Node 逐字一致。
process.env.ZCODE_TEST_WAIT_MS ??= "30000";
const nodeBundle = resolve("apps/zcode-cli/packages/cli/dist/zcode.cjs");
const CALLS: [string, unknown][] = [
  ["Read", {}],
  ["Read", { file_path: 5 }],
  ["Read", { file_path: "a.txt", offset: "x" }],
  ["Edit", { file_path: "a.txt" }],
  ["Bash", { command: "echo", timeout: "abc" }],
  ["Bash", { command: "echo", extra: true }],
  ["TodoWrite", { todos: "no" }],
  ["WebFetch", { url: 3 }],
  ["Read", { bogus: 1 }],
  ["TaskStop", { task_id: "x", extra: 1 }],
  ["Write", { file_path: "new.txt", content: "hello\n", junk: true }],
  ["Write", { file_path: "existing.txt", content: "x" }],
  ["Read", { file_path: "existing.txt", bogus: 1 }],
  ["Write", { file_path: "existing.txt", content: "one\ntwo\ntwo\n" }],
  ["Edit", { file_path: "existing.txt", old_string: "one", new_string: "uno" }],
  ["Edit", { file_path: "existing.txt", old_string: "two", new_string: "dos" }],
  ["Edit", { file_path: "existing.txt", old_string: "two", new_string: "dos", replace_all: true }],
  ["Edit", { file_path: "existing.txt", old_string: "missing", new_string: "x" }],
  ["Edit", { file_path: "existing.txt", old_string: "uno", new_string: "uno" }],
  ["Edit", { file_path: "existing.txt", old_string: "", new_string: "x" }],
  ["Edit", { file_path: "nope.txt", old_string: "a", new_string: "b" }],
  ["Read", { file_path: "existin.txt" }],
  ["Read", { file_path: "existing.txt", offset: 99 }],
  ["Read", { file_path: "." }],
  ["Read", { file_path: "empty.txt" }],
  ["Edit", { file_path: "fresh.txt", old_string: "", new_string: "created" }],
  ["TodoWrite", { todos: [{ content: "a", status: "pending", priority: "high", id: "1" }] }],
  ["TodoRead", {}],
  ["TaskOutput", { task_id: "nope" }],
  ["TaskStop", { task_id: "nope" }],
  ["Skill", { skill: "does-not-exist" }],
  ["WebFetch", { url: "not a url", prompt: "x" }],
  // runtime schema 认识 `pages`，定义里没有（fixture 模型不支持 PDF）：不丢弃，报多余参数。
  ["Read", { file_path: "existing.txt", pages: "1-2" }],
  // 策略拒绝发 TS reason 原文，而不是用户拒绝文案。
  ["ExitPlanMode", { plan: "p" }],
  // 只违反 refine：全部失败 issue 的 ZodError 文案。
  ["CronCreate", { prompt: "x", title: "t", delayMinutes: 5, cron: "* * * * *", recurring: true }],
  ["CronUpdate", { id: "nope", title: "t", maxRuns: null }],
  // AskUserQuestion 的 refine 失败：broker 直接拒绝，首条 issue 文案（顺序与 zod 遍历一致）。
  [
    "AskUserQuestion",
    {
      questions: [
        {
          question: "Q?",
          header: "H",
          multiSelect: false,
          options: [
            { label: "A", description: "a" },
            { label: "A", description: "b" },
          ],
        },
      ],
    },
  ],
  [
    "AskUserQuestion",
    {
      questions: [
        {
          question: "Q?",
          header: "H",
          multiSelect: false,
          options: [
            { label: "Other", description: "a", preview: "<html>x</html>" },
            { label: "B", description: "b" },
          ],
        },
      ],
    },
  ],
  // annotations 值只由 zod 检查；问题重复（输入层 refine）排在它之后。
  [
    "AskUserQuestion",
    {
      questions: [
        {
          question: "Q?",
          header: "H",
          multiSelect: false,
          options: [
            { label: "A", description: "a" },
            { label: "B", description: "b" },
          ],
        },
        {
          question: "Q?",
          header: "H2",
          multiSelect: false,
          options: [
            { label: "A", description: "a" },
            { label: "B", description: "b" },
          ],
        },
      ],
      annotations: { "Q?": { notes: null } },
    },
  ],
  // 先判模型能力，再校验入参。
  ["WebSearch", { query: "abc", allowed_domains: ["a.com"], blocked_domains: ["b.com"] }],
  // TS runtime schema 的宽松写法（preprocess/transform）：转换后执行，与 Node 相同。
  // 依赖读取状态的 Edit 放在 Bash 之前：Windows CI（8.3 短名临时目录）上 Node 在 Bash 之后会丢失读取状态
  // （原因未确认，见 rust-bash-model-content.md 实测记录）。
  ["Edit", { file_path: "existing.txt", old_string: "two", new_string: "2", replace_all: "yes" }],
  ["TaskOutput", { task_id: "nope", block: "false" }],
  ["Skill", { name: "does-not-exist" }],
  ["Bash", { command: "echo coerced", timeout: " 5000 ", run_in_background: "No" }],
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

test("built-in tool validation, file tool results and schema order match Node", async () => {
  const node = await observe("node");
  const rust = await observe("rust");
  assert.equal(node.seen.length, CALLS.length);
  for (const [index, [name, args]] of CALLS.entries()) {
    assert.equal(rust.seen[index], node.seen[index], `${name} ${JSON.stringify(args)}`);
  }
  // node_repl 只在 Node 的 bundle 中随官方插件出现，不属于内置工具。
  for (const [name, schema] of Object.entries(node.parameters)) {
    if (name.startsWith("mcp__")) continue;
    assert.equal(rust.parameters[name], schema, name);
  }
});
