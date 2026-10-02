import assert from "node:assert/strict";
import test from "node:test";
import { dirname, join, resolve } from "node:path";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { fixture, event, end, type Harness } from "./zcode-cli-rust-fixture.js";
import { configureRegistry } from "./zcode-cli-rust-registry-fixture.js";

// docs/specs/rust-hooks.md H1：会话级 hooks。第一轮：SessionStart 与 UserPromptSubmit 追加上下文，Stop 第一次要求续跑；
// 第二轮：UserPromptSubmit 阻止继续（不请求模型）。比对模型请求、hook stdin、`hookInvocation` 行、阻断错误与轮次收口。
process.env.ZCODE_TEST_WAIT_MS ??= "60000";
const nodeBundle = resolve("apps/zcode-cli/packages/cli/dist/zcode.cjs");

const hookScript = (log: string) => `
const fs = require("node:fs");
let raw = "";
process.stdin.on("data", (c) => (raw += c));
process.stdin.on("end", () => {
  const input = JSON.parse(raw);
  fs.appendFileSync(${JSON.stringify(log)}, JSON.stringify(input) + "\\n");
  const name = input.hookEventName;
  let out = {};
  if (name === "SessionStart") out = { hookSpecificOutput: { hookEventName: name, additionalContext: "session context" } };
  if (name === "UserPromptSubmit") {
    out = String(input.prompt).includes("block me")
      ? { continue: false, stopReason: "prompt rejected" }
      : { hookSpecificOutput: { hookEventName: name, additionalContext: "prompt context" } };
  }
  if (name === "Stop" && input.responseText === "first answer") out = { decision: "block", reason: "keep going" };
  process.stdout.write(JSON.stringify(out));
});
`;

async function observe(kind: "node" | "rust") {
  const requests: any[] = [];
  const respond = (req: any, res: any) => {
    requests.push(req);
    res.writeHead(200, { "content-type": "text/event-stream" });
    event(res, { content: ["first answer", "second answer"][requests.length - 1] ?? "third answer" });
    end(res, "stop");
  };
  const env = {
    ZCODE_OFFICIAL_PLUGINS_BASE_DIR: dirname(nodeBundle),
    ZCODE_PLUGIN_HOST_EXEC_PATH: process.execPath, ZCODE_PLUGIN_HOST_ENTRYPOINT: nodeBundle };
  const f =
    kind === "node"
      ? await fixture({
          command: process.execPath,
          args: ({ cwd }) => [nodeBundle, "app-server", "--stdio", "--cwd", cwd],
          registry: true,
          respond,
          mode: "yolo",
          env,
        })
      : await fixture({ registry: true, respond, mode: "yolo", env });
  try {
    await configureRegistry(f, false);
    const log = join(f.root, "hooks.log");
    const script = join(f.root, "hook.cjs");
    await writeFile(script, hookScript(log));
    const hook = [{ hooks: [{ type: "process", command: process.execPath, args: [script] }] }];
    await mkdir(join(f.root, ".zcode", "cli"), { recursive: true });
    await writeFile(
      join(f.root, ".zcode", "cli", "config.json"),
      JSON.stringify({ hooks: { enabled: true, events: { SessionStart: hook, UserPromptSubmit: hook, Stop: hook } } }),
    );
    const h: Harness = f.start();
    const sessionId = await h.create();
    await h.subscribe(`conversation/${sessionId}`);
    let after = h.messages.length;
    await h.command(h.envelope("sendText", sessionId, { text: "hello", mode: "yolo" }));
    await h.completed(sessionId, after);
    after = h.messages.length;
    await h.command(h.envelope("sendText", sessionId, { text: "please block me", mode: "yolo" }));
    await h.completed(sessionId, after);
    after = h.messages.length;
    await h.command(h.envelope("sendText", sessionId, { text: "after", mode: "yolo" }));
    await h.completed(sessionId, after);
    await new Promise((done) => setTimeout(done, 300));
    await h.close();
    const rows = new Map<number, any>();
    let lastError: any;
    for (const m of h.messages) {
      for (const delta of m.params?.frame?.payload?.deltas ?? []) {
        if (delta.row) rows.set(delta.row.rowId, delta.row);
        if (delta.patch?.control && "lastError" in delta.patch.control) lastError = delta.patch.control.lastError;
      }
    }
    const stdin = (await readFile(log, "utf8").catch(() => ""))
      .split("\n")
      .filter(Boolean)
      .map((line) => {
        const input = JSON.parse(line);
        return {
          keys: Object.keys(input).sort(),
          hookEventName: input.hookEventName,
          source: input.source,
          prompt: input.prompt,
          responseText: input.responseText,
          stopHookActive: input.stopHookActive,
          toolCallCount: input.toolCallCount,
          model: typeof input.model,
        };
      });
    const rootText = JSON.stringify(f.root).slice(1, -1);
    const scrub = (value: unknown) =>
      JSON.parse(JSON.stringify(value).replaceAll(rootText, "<root>").replace(/Today's date is [^.]*\./g, "Today's date is <date>."));
    return {
      requests: requests.length,
      messages: scrub(requests.map((r) => r.messages.filter((m: any) => m.role !== "system"))),
      stdin,
      rows: [...rows.values()]
        .sort((a, b) => a.rowId - b.rowId)
        .map((row) =>
          row.kind === "hookInvocation"
            ? { kind: row.kind, hookEventName: row.hookEventName, state: row.state, lane: row.lane,
                executions: row.executions.map((e: any) => [e.state, e.outcome, e.blockReason, e.didExecute]) }
            : { kind: row.kind, text: row.text ?? row.content?.text, status: row.status },
        ),
      lastError: lastError && { code: lastError.code, message: lastError.message, detail: lastError.detail },
      schemaErrors: h.schemaErrors,
    };
  } finally {
    await f.close();
  }
}

test("Node and Rust run session-level hooks the same way", async () => {
  const node = await observe("node");
  if (process.env.ZCODE_DUMP) console.log(JSON.stringify(node, null, 1));
  const rust = await observe("rust");
  assert.deepEqual(node.schemaErrors, []);
  assert.equal(node.requests, 3);
  for (const key of Object.keys(node)) {
    assert.deepEqual((rust as any)[key], (node as any)[key], key);
  }
});
