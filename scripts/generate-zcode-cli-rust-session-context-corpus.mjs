// Run with node --import tsx. 以 TS ReadSessionContext handler 为 oracle：假 session store 提供 MessageWithParts，
// 假模型按脚本应答并记录每次辅助调用。Rust domain::session_context 以同样输入逐条比对
// （活跃消息选择、素材、辅助调用、输出与模型可见内容）。见 docs/specs/rust-read-session-context.md。
import { createHash } from "node:crypto";
import { readFile, writeFile } from "node:fs/promises";
import { readSessionContextToolEntry } from "../apps/zcode-cli/packages/core/src/tool/handlers/read-session-context.ts";

let clock = Date.UTC(2026, 0, 2, 3, 4, 5);
const msg = (id, role, parts, extra = {}) => ({
  info: { id, sessionID: "sess_target", role, time: { created: (clock += 61_000) }, ...extra },
  parts: parts.map((part, index) => ({
    id: `${id}-p${index}`,
    sessionID: "sess_target",
    messageID: id,
    ...part,
  })),
});
const text = (value, extra = {}) => ({ type: "text", text: value, ...extra });
const tool = (name, status, state) => ({
  type: "tool",
  tool: name,
  callID: `c-${name}`,
  state: { status, ...state },
});
const stepFinish = (reason) => ({
  type: "step-finish",
  reason,
  cost: 0,
  tokens: { input: 1, output: 1, reasoning: 0, cache: { read: 0, write: 0 } },
});

const basic = () => [
  msg("m1", "user", [text("What is in src/app.ts? 修复登录问题")]),
  msg("m2", "assistant", [
    { type: "step-start" },
    tool("Read", "completed", {
      input: { file_path: "src/app.ts", limit: 20 },
      output: "1\texport const app = 1;",
      title: "Read",
      metadata: {},
    }),
    text("Let me read src/app.ts."),
    stepFinish("tool-calls"),
  ]),
  msg("m3", "assistant", [
    { type: "step-start" },
    text("app.ts exports app. 登录问题在 auth.ts。"),
    stepFinish("stop"),
  ]),
];

const variety = () => [
  msg("v1", "user", [
    text("Please check the attachment and fix auth"),
    text("<system-reminder>\nhidden\n</system-reminder>"),
    text("synthetic todo", { synthetic: true, metadata: { source: "todo_reminder" } }),
    text("synthetic kept", { synthetic: true, metadata: { source: "user_upload" } }),
    text("ignored", { ignored: true }),
    {
      type: "file",
      mime: "text/plain",
      filename: "notes.txt",
      url: "file:///n",
      source: {
        type: "file",
        path: "/w/notes.txt",
        text: { value: "note body", start: 0, end: 9 },
      },
    },
    { type: "file", mime: "image/png", filename: "shot.png", url: "data:x" },
    {
      type: "file",
      mime: "text/markdown",
      url: "mcp://r",
      source: {
        type: "resource",
        uri: "mcp://server/res",
        clientName: "s",
        text: { value: "resource text", start: 0, end: 13 },
      },
      metadata: { preview: { text: "preview wins" } },
    },
    { type: "agent", name: "Explore" },
  ]),
  msg("v2", "user", [text("model only reminder")], { visibility: "model-only" }),
  msg("v3", "assistant", [
    { type: "step-start" },
    { type: "reasoning", text: "thinking", time: { start: 1 } },
    tool("Bash", "error", {
      input: { command: "npm test", description: "run" },
      error: "exit 1\n".repeat(400),
      time: { start: 1, end: 2 },
    }),
    tool("Grep", "pending", { input: {}, raw: '{"pattern":"au' }),
    tool("Edit", "running", {
      input: { file_path: "auth.ts", old_string: "a", new_string: "b" },
      time: { start: 1 },
    }),
    {
      type: "subtask",
      prompt: "explore auth",
      description: "Explore auth",
      agent: "Explore",
      command: "/explore",
    },
    { type: "subtask", prompt: "p".repeat(3100), description: "Long", agent: "general-purpose" },
    { type: "patch", hash: "h", files: ["auth.ts", "login.ts"] },
    {
      type: "retry",
      attempt: 2,
      error: { name: "APIError", data: { message: "overloaded" } },
      time: { created: 1 },
    },
    text("x".repeat(3100)),
    stepFinish("stop"),
  ]),
  msg("v4", "assistant", [text("   ")]),
];

const compacted = () => [
  msg("c1", "user", [text("old question about billing")]),
  msg("c2", "assistant", [text("old answer about billing"), stepFinish("stop")]),
  msg("c3", "user", [text("preserved question about auth")]),
  msg("c4", "assistant", [text("preserved answer about auth"), stepFinish("stop")]),
  msg("c5", "user", [
    {
      type: "compaction",
      auto: true,
      reason: "context limit",
      timelineText: "Conversation compacted",
      compactBoundary: {
        preservedSegment: { headMessageId: "c3", tailMessageId: "c4", anchorMessageId: "c6" },
      },
    },
  ]),
  msg("c6", "assistant", [text("Summary: we discussed billing and auth."), stepFinish("stop")]),
  msg("c7", "user", [text("new question about auth tokens")]),
  msg("c8", "assistant", [text("tokens live in auth.ts"), stepFinish("stop")]),
];

const large = () => {
  const messages = [];
  for (let i = 0; i < 60; i++) {
    const topic =
      i % 7 === 0 ? "database migration" : i % 5 === 0 ? "auth token refresh" : "misc chatter";
    messages.push(msg(`L${i}u`, "user", [text(`Q${i}: ${topic} ${"lorem ipsum ".repeat(120)}`)]));
    messages.push(
      msg(`L${i}a`, "assistant", [
        tool("Read", "completed", {
          input: { file_path: `f${i}.ts` },
          output: `content ${i} ${"z".repeat(1700)}`,
          title: "Read",
          metadata: {},
        }),
        text(`A${i}: about ${topic}. ${"dolor sit ".repeat(150)}`),
        stepFinish("stop"),
      ]),
    );
  }
  return messages;
};

const fixtures = { basic: basic(), variety: variety(), compacted: compacted(), large: large() };
// 超过 2000 个 UTF-16 码元的字符串以摘要比对（sha256(UTF-8) + 长度 + 首尾），控制语料体积。
const digest = (value) =>
  typeof value === "string" && value.length > 2000
    ? {
        sha256: createHash("sha256").update(value, "utf8").digest("hex"),
        length: value.length,
        head: value.slice(0, 300),
        tail: value.slice(-300),
      }
    : value;
const session = {
  id: "sess_target",
  title: "Target session",
  directory: "/work/app",
  projectID: "p",
  slug: "t",
  version: "1",
  time: { created: 1, updated: 2 },
};
const scenarios = [
  {
    name: "local-only",
    fixture: "basic",
    input: { sessionId: "sess_target", query: "src/app.ts" },
    model: null,
  },
  {
    name: "lite-single",
    fixture: "basic",
    input: { sessionId: "sess_target", query: "登录问题 app.ts", maxTokens: 900 },
    model: ["  lite answer  "],
  },
  {
    name: "lite-no-relevant",
    fixture: "basic",
    input: { sessionId: "sess_target", query: "billing" },
    model: [" no_relevant_context "],
  },
  {
    name: "lite-empty",
    fixture: "basic",
    input: { sessionId: "sess_target", query: "billing", strategy: "handoff" },
    model: [""],
  },
  {
    name: "lite-error",
    fixture: "basic",
    input: { sessionId: "sess_target", query: "app" },
    model: [{ error: "provider exploded" }],
  },
  {
    name: "variety-local",
    fixture: "variety",
    input: { sessionId: "sess_target", query: "auth fix", strategy: "relevant", maxTokens: 12000 },
    model: null,
  },
  {
    name: "variety-handoff",
    fixture: "variety",
    input: { sessionId: "sess_target", query: "continue", strategy: "handoff", maxTokens: 500 },
    model: null,
  },
  {
    name: "compacted",
    fixture: "compacted",
    input: { sessionId: "sess_target", query: "auth" },
    model: null,
    path: "/work/app/.worktrees/x",
  },
  {
    name: "large-relevant",
    fixture: "large",
    input: { sessionId: "sess_target", query: "auth token refresh", maxTokens: 3000 },
    model: [
      "NO_RELEVANT_CONTEXT",
      "chunk two notes",
      "chunk three notes",
      "NO_RELEVANT_CONTEXT",
      "chunk five notes",
      "synthesized answer",
    ],
  },
  {
    name: "large-single-chunk",
    fixture: "large",
    input: { sessionId: "sess_target", query: "database migration", strategy: "handoff" },
    model: [
      "short",
      "NO_RELEVANT_CONTEXT",
      "NO_RELEVANT_CONTEXT",
      "NO_RELEVANT_CONTEXT",
      "NO_RELEVANT_CONTEXT",
    ],
  },
  {
    name: "large-all-irrelevant",
    fixture: "large",
    input: { sessionId: "sess_target", query: "kubernetes" },
    model: Array(5).fill("NO_RELEVANT_CONTEXT"),
  },
  {
    name: "large-local",
    fixture: "large",
    input: { sessionId: "sess_target", query: "misc chatter", strategy: "relevant", maxTokens: 20 },
    model: null,
  },
  {
    name: "empty-session",
    fixture: "empty",
    input: { sessionId: "sess_target", query: "anything" },
    model: ["unused"],
  },
  {
    name: "not-found",
    fixture: null,
    input: { sessionId: "sess_missing", query: "x", strategy: "handoff" },
    model: null,
  },
];

const cases = [];
for (const scenario of scenarios) {
  const calls = [];
  const script = [...(scenario.model ?? [])];
  const model = scenario.model
    ? {
        optionSpecs: {
          reasoningLevel: { values: ["low", "high"] },
          maxOutputTokens: { max: 8000 },
        },
        async generateText(request) {
          calls.push({
            messages: request.messages,
            maxOutputTokens: request.options.maxOutputTokens,
            reasoningLevel: request.options.reasoningLevel,
          });
          const next = script.shift() ?? "";
          if (typeof next === "object") throw new Error(next.error);
          return { text: next };
        },
      }
    : undefined;
  scenario.messages =
    scenario.fixture === null
      ? null
      : scenario.fixture === "empty"
        ? []
        : fixtures[scenario.fixture];
  const target =
    scenario.messages === null
      ? null
      : { ...session, ...(scenario.path ? { path: scenario.path } : {}) };
  const context = {
    toolCallId: "t",
    traceId: "trace",
    sessionId: "sess_current",
    abortSignal: new AbortController().signal,
    model,
    sessionStore: {
      async getSession() {
        return target;
      },
      async messages() {
        return scenario.messages ?? [];
      },
    },
  };
  const output = await readSessionContextToolEntry.handler(
    readSessionContextToolEntry.runtimeInputSchema.parse(scenario.input),
    context,
  );
  cases.push({
    name: scenario.name,
    session: target,
    fixture: scenario.fixture,
    input: scenario.input,
    model: scenario.model,
    calls: calls.map((call) => ({
      ...call,
      messages: call.messages.map((m) => ({ role: m.role, content: digest(m.content) })),
    })),
    output: { ...JSON.parse(JSON.stringify(output)), content: digest(output.content) },
    modelContent: digest(readSessionContextToolEntry.formatModelContent(output)),
  });
}

const content = `${JSON.stringify({ fixtures, cases })}\n`;
const target = new URL(
  "../apps/zcode-cli-rust/crates/domain/tests/fixtures/session_context_corpus.json",
  import.meta.url,
);
if (process.argv.includes("--check")) {
  if ((await readFile(target, "utf8")) !== content)
    throw new Error("Rust session context corpus differs from TS");
} else await writeFile(target, content);
