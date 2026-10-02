import assert from "node:assert/strict";
import test from "node:test";
import { join, resolve } from "node:path";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { fixture, event, end, type Harness } from "./zcode-cli-rust-fixture.js";
import { configureRegistry } from "./zcode-cli-rust-registry-fixture.js";

// docs/specs/rust-hooks.md H1：用户配置的 PreToolUse / PostToolUse hooks。第一条 Bash 被 PreToolUse 拒绝（带追加上下文），
// 第二条放行并由 PostToolUse 追加上下文。比对模型可见的工具结果、hook 收到的 stdin 与 `hookInvocation` 行。
process.env.ZCODE_TEST_WAIT_MS ??= "60000";
const nodeBundle = resolve("apps/zcode-cli/packages/cli/dist/zcode.cjs");

const hookScript = (log: string) => `
const fs = require("node:fs");
let raw = "";
process.stdin.on("data", (c) => (raw += c));
process.stdin.on("end", () => {
  const input = JSON.parse(raw);
  fs.appendFileSync(${JSON.stringify(log)}, JSON.stringify(input) + "\\n");
  const blocked = String(input.toolInput?.command ?? "").includes("blocked");
  const out = input.hookEventName === "PreToolUse"
    ? { hookSpecificOutput: { hookEventName: "PreToolUse", additionalContext: "pre context",
        ...(blocked ? { permissionDecision: "deny", permissionDecisionReason: "blocked by policy" } : {}) } }
    : { hookSpecificOutput: { hookEventName: input.hookEventName, additionalContext: "post context" } };
  process.stdout.write(JSON.stringify(out));
});
`;

const call = (id: string, command: string) => ({
  tool_calls: [
    { index: 0, id, type: "function", function: { name: "Bash", arguments: JSON.stringify({ command }) } },
  ],
});

async function observe(kind: "node" | "rust") {
  const requests: any[] = [];
  const respond = (req: any, res: any) => {
    requests.push(req);
    res.writeHead(200, { "content-type": "text/event-stream" });
    const steps = [call("bash-1", "echo blocked"), call("bash-2", "echo ok")];
    const step = steps[requests.length - 1];
    if (step) {
      event(res, step);
      end(res, "tool_calls");
    } else {
      event(res, { content: "done" });
      end(res, "stop");
    }
  };
  const env = { ZCODE_PLUGIN_HOST_EXEC_PATH: process.execPath, ZCODE_PLUGIN_HOST_ENTRYPOINT: nodeBundle };
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
    const hook = { type: "process", command: process.execPath, args: [script] };
    await mkdir(join(f.root, ".zcode", "cli"), { recursive: true });
    await writeFile(
      join(f.root, ".zcode", "cli", "config.json"),
      JSON.stringify({
        hooks: {
          enabled: true,
          events: {
            PreToolUse: [{ matcher: "Bash", hooks: [hook] }],
            PostToolUse: [{ matcher: "Bash", hooks: [hook] }],
          },
        },
      }),
    );
    const h: Harness = f.start();
    const sessionId = await h.create();
    await h.subscribe(`conversation/${sessionId}`);
    await h.command(h.envelope("sendText", sessionId, { text: "run it", mode: "yolo" }));
    await h.completed(sessionId);
    await h.close();
    const rows = new Map<string, any>();
    for (const m of h.messages) {
      for (const delta of m.params?.frame?.payload?.deltas ?? []) {
        if (delta.row?.kind === "hookInvocation") rows.set(delta.row.hookInvocationId, delta.row);
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
          toolName: input.toolName,
          toolCallId: input.toolCallId,
          toolInput: input.toolInput,
          mode: input.mode,
          riskLevel: input.riskLevel,
          sideEffectScope: input.sideEffectScope,
          sameSession: input.sessionId === sessionId,
          toolResponse: input.toolResponse,
        };
      });
    const toolResult = (index: number, id: string) =>
      requests[index]?.messages?.find((m: any) => m.tool_call_id === id)?.content;
    return {
      blocked: toolResult(1, "bash-1"),
      allowed: toolResult(2, "bash-2"),
      stdin,
      rows: [...rows.values()].map((row) => ({
        hookEventName: row.hookEventName,
        hookCount: row.hookCount,
        state: row.state,
        lane: row.lane,
        anchorToolCallId: row.anchorToolCallId,
        hasEnd: typeof row.endedAt === "number",
        executions: row.executions.map((e: any) => ({
          didExecute: e.didExecute,
          state: e.state,
          outcome: e.outcome,
          blockReason: e.blockReason,
          displayName: e.displayName,
          sourceKind: e.sourceKind,
          toolName: e.toolName,
          hasDuration: typeof e.durationMs === "number",
        })),
      })),
      schemaErrors: h.schemaErrors,
    };
  } finally {
    await f.close();
  }
}

test("Node and Rust run tool hooks the same way", async () => {
  const node = await observe("node");
  const rust = await observe("rust");
  assert.deepEqual(node.schemaErrors, []);
  assert.match(String(node.blocked), /blocked by policy/);
  assert.match(String(node.allowed), /post context/);
  assert.equal(node.rows.length, 3);
  assert.equal(node.stdin.length, 3);
  for (const key of Object.keys(node)) {
    assert.deepEqual((rust as any)[key], (node as any)[key], key);
  }
});
