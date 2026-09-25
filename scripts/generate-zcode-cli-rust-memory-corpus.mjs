// Run with node --import tsx. 项目记忆纯规则的 TS oracle（docs/specs/rust-project-memory.md）：
// 索引格式化、Memory 段、manifest 与提取提示词、受限 agent 工具策略、提取判定。--check 防漂移。
// 语料只用 POSIX 风格的字面路径且不做 path.join，使三平台输出一致；记忆根 hash 与 MEMORY.md
// 路径拼接依赖平台，由 App 差分在各平台覆盖。
import { readFile, writeFile } from "node:fs/promises";
import { formatProjectMemoryIndexContent } from "../apps/zcode-cli/packages/core/src/memory/index-content.ts";
import { buildMemorySection } from "../apps/zcode-cli/packages/core/src/context/sections/memory.ts";
import {
  buildMemoryExtractionPrompt,
  createMemoryExtractionScheduler,
} from "../apps/zcode-cli/packages/core/src/memory/extraction.ts";
import {
  formatMemoryManifest,
  scanMemoryManifest,
} from "../apps/zcode-cli/packages/core/src/memory/recall/manifest.ts";
import { runMemoryAgentLoop } from "../apps/zcode-cli/packages/core/src/memory/memory-agent-loop.ts";
import { stampMemoryOriginSessionId } from "../apps/zcode-cli/packages/core/src/memory/origin-session.ts";

const rootDir = "/mem";
const inRoot = (p) => `${rootDir}/${p}`;
const workspace = "/work/repo";

// ---- 索引格式化与 Memory 段 ----
const long = Array.from({ length: 230 }, (_, i) => `- [m${i}](m${i}.md) — hook ${i}`).join("\n");
const wide = Array.from({ length: 50 }, (_, i) => `- ${"x".repeat(600)} ${i}`).join("\n");
const indexes = [
  "- [A](a.md) — first",
  "---\nname: x\n---\n- [A](a.md)",
  "---  \n\nk: v\n---\n\n- after frontmatter",
  "<!-- top comment -->\n- [A](a.md)",
  "- item\n<!-- inline in list -->\n- next",
  "text\n\n<!-- a -->\n<!-- b -->\n\nmore",
  "```\n<!-- in code -->\n```\n<!-- out -->",
  "> <!-- quoted -->\n\n  <!-- indented two -->\n\n    <!-- code indent -->",
  "<!-- keep -->trailing text\n",
  "para line\n<!-- interrupts -->\nafter",
  "<!--\nmulti\nline\n-->\nrest",
  "- item\n  <!-- nested in item -->\n- next",
  "1. one\n   <!-- ordered nested -->\n\n<!-- after list -->\nz",
  "~~~\n<!-- tilde fence -->\n~~~~\n<!-- gone -->",
  "> quote\n<!-- lazy -->\nend",
  "* star\n\n  <!-- item paragraph -->\n\nx",
  "   ",
  long,
  wide,
  `${long}\n${wide}`,
].map((content) => ({ content, formatted: formatProjectMemoryIndexContent(content) }));
const section = buildMemorySection(
  "/store/cli/memories/projects/repo-0123456789abcdef/memory",
).content;

// ---- manifest 与提取提示词 ----
const files = {
  "a.md": "---\nname: a\ndescription: Alpha fact\nmetadata:\n  type: user\n---\nbody",
  "nested/b.md": '---\ndescription: "Quoted: beta"\ntype: feedback\n---\n',
  "c.md": "no frontmatter",
  "d.md": "---\ndescription: 42\nmetadata:\n  type: nonsense\n---\n",
  "e.md": "---\r\ndescription: crlf desc # comment\r\n---\r\n",
  "f.md": "---\ndescription: >-\n  folded\n  text\n---\n",
  "g.md": "---\ndescription: 'single ''quoted'''\nmetadata: {type: reference}\n---\n",
  "MEMORY.md": "- index",
  "h.txt": "ignored",
};
const mtimes = Object.fromEntries(Object.keys(files).map((p, i) => [p, 1767000000000 - i * 60000]));
const fakeFs = {
  listDirectory: async ({ path }) => {
    const prefix = path === rootDir ? "" : `${path.slice(rootDir.length + 1)}/`;
    const names = new Set();
    for (const p of Object.keys(files)) {
      if (!p.startsWith(prefix)) continue;
      const rest = p.slice(prefix.length);
      names.add(rest.includes("/") ? `${rest.split("/")[0]}/` : rest);
    }
    return {
      entries: [...names].map((n) =>
        n.endsWith("/")
          ? { kind: "directory", path: inRoot(`${prefix}${n.slice(0, -1)}`) }
          : { kind: "file", path: inRoot(`${prefix}${n}`) },
      ),
    };
  },
  stat: async ({ path }) => ({ kind: "file", mtimeMs: mtimes[path.slice(rootDir.length + 1)] }),
  readTextFileRange: async ({ path }) => ({
    content: files[path.slice(rootDir.length + 1)].split("\n").slice(0, 30).join("\n"),
  }),
};
const manifest = (await scanMemoryManifest({ fileSystem: fakeFs, rootDir })).map(
  ({ filePath: _filePath, ...entry }) => ({
    ...entry,
    filename: entry.filename.replaceAll("\\", "/"),
  }),
);
const extractionPrompts = [
  { manifest: [], messageCount: 4 },
  { manifest, messageCount: 12 },
].map((input) => ({
  ...input,
  formattedManifest: formatMemoryManifest(input.manifest),
  prompt: buildMemoryExtractionPrompt(input),
}));

// ---- 受限 agent 工具策略 ----
const toolCalls = [
  ["Read", { file_path: inRoot("a.md") }],
  ["Grep", { pattern: "x" }],
  ["Glob", { pattern: "*.md" }],
  ["Write", { file_path: inRoot("a.md"), content: "x" }],
  ["Write", { file_path: inRoot("a.txt"), content: "x" }],
  ["Write", { file_path: inRoot("skills/a.md"), content: "x" }],
  ["Write", { file_path: inRoot("Config/a.md"), content: "x" }],
  ["Write", { file_path: inRoot("x.git./a.md"), content: "x" }],
  ["Write", { file_path: inRoot("hooks:stream/a.md"), content: "x" }],
  ["Edit", { file_path: inRoot("../escape.md"), old_string: "a", new_string: "b" }],
  ["Edit", { file_path: "/elsewhere/a.md", old_string: "a", new_string: "b" }],
  ["Write", { file_path: "relative.md", content: "x" }],
  ["Write", { file_path: rootDir, content: "x" }],
  ["Bash", { command: "ls -la" }],
  ["Bash", { command: "cat a.md | head -3" }],
  ["Bash", { command: "echo hi > out.txt" }],
  ["Bash", { command: `rm ${inRoot("a.md")}` }],
  ["Bash", { command: `rm -f -- ${inRoot("a.md")} ${inRoot("b.md")}` }],
  ["Bash", { command: `rm -rf ${inRoot("a.md")}` }],
  ["Bash", { command: `rm --recursive ${inRoot("a.md")}` }],
  ["Bash", { command: `rm ${inRoot("*.md")}` }],
  ["Bash", { command: "rm a.md" }],
  ["Bash", { command: `rm ${inRoot("a.txt")}` }],
  ["Bash", { command: `FOO=1 rm ${inRoot("a.md")}` }],
  ["Bash", { command: `rm ${inRoot("a.md")} && ls` }],
  ["Bash", { command: "rm" }],
  ["Bash", {}],
  ["Agent", { prompt: "x" }],
  ["mcp__srv__tool", {}],
  ["WebFetch", { url: "https://example.com" }],
  ["Unknown", {}],
  ["TodoWrite", { todos: [] }],
];
const toolContracts = [
  "Read",
  "Grep",
  "Glob",
  "Write",
  "Edit",
  "Bash",
  "Agent",
  "mcp__srv__tool",
  "TodoWrite",
].map((name) => ({ name, description: name, inputSchema: { type: "object" } }));
toolContracts.push({
  name: "WebFetch",
  description: "WebFetch",
  inputSchema: { type: "object" },
  sideEffectScope: "network",
});
const policies = [];
for (const [name, input] of toolCalls) {
  let calls = 0;
  let executed = false;
  const result = await runMemoryAgentLoop({
    executeTool: async () => {
      executed = true;
      return { content: "ok", status: "success" };
    },
    maxTurns: 2,
    messages: [{ role: "user", content: "go" }],
    model: {
      properties: { inputFormat: { text: true } },
      optionSpecs: { reasoningLevel: { values: ["low"] }, maxOutputTokens: { max: 8000 } },
      generateText: async () =>
        calls++ === 0
          ? { text: "", toolCalls: [{ id: "t1", name, input }] }
          : { text: "done", toolCalls: [] },
    },
    rootDir,
    tools: toolContracts,
    workingDirectory: workspace,
    workspaceRoot: workspace,
  });
  const toolMessage = result.messages.find((m) => m.role === "tool");
  policies.push({
    name,
    input,
    allowed: executed,
    ...(executed ? {} : { reason: toolMessage.content }),
  });
}

// ---- 提取判定（抽象消息：user 文本 / assistant 文本或写入路径）----
const decisionCases = [
  { name: "prose", messages: [{ user: "please remember that I prefer tabs" }] },
  { name: "short", messages: [{ user: "ok thanks" }] },
  { name: "whitespace-words", messages: [{ user: "  a\tb\nc  " }] },
  { name: "synthetic", messages: [{ user: "a b c d e", synthetic: true }] },
  { name: "model-only", messages: [{ user: "a b c d e", modelOnly: true }] },
  {
    name: "direct-write",
    messages: [{ user: "remember this fact please" }, { write: inRoot("a.md") }],
  },
  {
    name: "outside-write",
    messages: [{ user: "remember this fact please" }, { write: "/work/repo/a.md" }],
  },
  {
    name: "cursor-skips-old-prose",
    cursorAfter: 1,
    messages: [{ user: "old prose with many words" }, { assistant: "ok" }, { user: "hi" }],
  },
  {
    name: "cursor-old-write-ignored",
    cursorAfter: 1,
    messages: [{ user: "x" }, { write: inRoot("a.md") }, { user: "new prose with four words" }],
  },
];
let messageSeq = 0;
const toMessages = (messages) =>
  messages.map((m) => {
    const id = `m${messageSeq++}`;
    if (m.user !== undefined)
      return {
        info: {
          id,
          role: "user",
          ...(m.synthetic ? { synthetic: true } : {}),
          ...(m.modelOnly ? { visibility: "model-only" } : {}),
        },
        parts: [{ type: "text", text: m.user }],
      };
    return {
      info: { id, role: "assistant" },
      parts: m.write
        ? [{ type: "tool", tool: "Write", state: { input: { file_path: m.write } } }]
        : [{ type: "text", text: m.assistant }],
    };
  });
const decisions = [];
for (const c of decisionCases) {
  const durable = toMessages(c.messages);
  const runs = [];
  const scheduler = createMemoryExtractionScheduler(async (input) => {
    runs.push(input.messageCount);
    return "success";
  });
  const snapshot = (end) => ({
    boundaryMessageId: durable[end].info.id,
    durableMessages: durable.slice(0, end + 1),
    memoryRoot: rootDir,
    workingDirectory: workspace,
    workspaceRoot: workspace,
  });
  if (c.cursorAfter !== undefined) {
    // 先以 cursorAfter 为边界跑一次推进游标，再对完整快照判定。
    scheduler.schedule(snapshot(c.cursorAfter));
    await scheduler.drain();
    runs.length = 0;
  }
  scheduler.schedule(snapshot(durable.length - 1));
  await scheduler.drain();
  decisions.push({ ...c, run: runs.length > 0, ...(runs.length ? { messageCount: runs[0] } : {}) });
}

// ---- 写入时补写来源会话 ----
const stamps = [
  ["/mem/a.md", "---\nname: a\ndescription: d\nmetadata:\n  type: user\n---\nbody"],
  ["/mem/a.md", "---\nname: a\n---\nbody"],
  ["/mem/a.md", "---\nmetadata:\n    type: user\n    node_type: other\n---\n"],
  ["/mem/a.md", "---\nmetadata:\n  originSessionId: sess_old\n---\nx"],
  ["/mem/a.md", "---\nmetadata:\n  originSessionId:\n---\nx"],
  ["/mem/a.md", "---\nmetadata: {type: reference}\n---\nx"],
  ["/mem/a.md", "---\r\nname: a\r\nmetadata:\r\n  type: user\r\n---\r\nbody"],
  ["/mem/a.md", "﻿---\nname: a\n---"],
  ["/mem/a.md", "no frontmatter"],
  ["/mem/a.txt", "---\nname: a\n---\n"],
  ["/other/a.md", "---\nname: a\n---\n"],
  ["/mem/a.md", "---\nmetadata: plain\n---\n"],
  ["/mem/a.md", "---\nname: a\n# comment\nmetadata:\n  type: user\n  tags:\n    - x\n---\n"],
  ["/mem/nested/b.md", '---\ndescription: "quoted: yes"\n---\nbody\n---\nnot frontmatter'],
  ["/mem/a.md", "---\nbad: a: b\n---\n"],
].map(([filePath, content]) => ({
  filePath,
  content,
  stamped: stampMemoryOriginSessionId({
    content,
    filePath,
    memoryRoot: rootDir,
    sessionId: "sess_1",
  }),
}));

const content = `${JSON.stringify(
  {
    indexes,
    section,
    manifestFiles: files,
    mtimes,
    manifest,
    extractionPrompts,
    rootDir,
    policies,
    decisions,
    stamps,
  },
  null,
  1,
)}\n`;
const target = new URL(
  "../apps/zcode-cli-rust/crates/domain/tests/fixtures/memory_corpus.json",
  import.meta.url,
);
if (process.argv.includes("--check")) {
  const current = await readFile(target, "utf8").catch(() => "");
  if (current !== content) throw new Error("Rust memory corpus differs from TS");
} else await writeFile(target, content);
