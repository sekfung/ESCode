import assert from "node:assert/strict";
import test from "node:test";
import { dirname, join, resolve } from "node:path";
import { mkdir, mkdtemp, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { fixture, event, end, type Harness } from "./escode-cli-rust-fixture.js";
import { configureRegistry } from "./escode-cli-rust-registry-fixture.js";

// docs/specs/rust-hooks.md H3：`ESCODE_MESSAGE_ENABLED` 灰度下的会话 mailbox 内部 hooks。开轮前的未读消息由
// UserPromptSubmit 并入上下文；轮中到达的消息在工具之后（PostToolUse）作为本轮 guide 输入。比对模型请求。
process.env.ESCODE_TEST_WAIT_MS ??= "60000";
const nodeBundle = resolve("apps/escode-cli/packages/cli/dist/escode.cjs");

async function observe(kind: "node" | "rust") {
  const requests: any[] = [];
  let mailbox = "";
  let sessionId = "";
  const deliver = async (name: string, content: string) => {
    const dir = join(mailbox, sessionId, "unread");
    await mkdir(dir, { recursive: true });
    await writeFile(
      join(dir, `${name}.json`),
      JSON.stringify({ version: 1, messageId: name, fromSessionId: "sess_peer", toSessionId: sessionId, content, createdAt: "2026-01-01T00:00:00.000Z" }),
    );
  };
  const respond = async (req: any, res: any) => {
    requests.push(req);
    if (requests.length === 1) await deliver("m2", "second note");
    res.writeHead(200, { "content-type": "text/event-stream" });
    if (requests.length === 1) {
      event(res, {
        tool_calls: [{ index: 0, id: "bash-1", type: "function", function: { name: "Bash", arguments: JSON.stringify({ command: "echo ok" }) } }],
      });
      end(res, "tool_calls");
    } else {
      event(res, { content: `answer ${requests.length}` });
      end(res, "stop");
    }
  };
  mailbox = await mkdtemp(join(tmpdir(), "escode-mailbox-"));
  const env = {
    ESCODE_OFFICIAL_PLUGINS_BASE_DIR: dirname(nodeBundle),
    ESCODE_PLUGIN_HOST_EXEC_PATH: process.execPath,
    ESCODE_PLUGIN_HOST_ENTRYPOINT: nodeBundle,
    ESCODE_MESSAGE_ENABLED: "1",
  };
  const f =
    kind === "node"
      ? await fixture({
          command: process.execPath,
          args: ({ cwd }) => [nodeBundle, "app-server", "--stdio", "--cwd", cwd],
          registry: true,
          respond,
          mode: "yolo",
          env: { ...env, ESCODE_MAILBOX_ROOT: mailbox },
        })
      : await fixture({ registry: true, respond, mode: "yolo", env: { ...env, ESCODE_MAILBOX_ROOT: mailbox } });
  try {
    await configureRegistry(f, false);
    const h: Harness = f.start();
    sessionId = await h.create();
    await h.subscribe(`conversation/${sessionId}`);
    await deliver("m1", "first note");
    await h.command(h.envelope("sendText", sessionId, { text: "hello", mode: "yolo" }));
    await h.completed(sessionId);
    await new Promise((done) => setTimeout(done, 300));
    await h.close();
    const scrub = (value: unknown) =>
      JSON.parse(
        JSON.stringify(value)
          .replaceAll(sessionId, "<session>")
          .replaceAll(JSON.stringify(f.root).slice(1, -1), "<root>")
          .replace(/Today's date is [^.]*\./g, "Today's date is <date>."),
      );
    return {
      requests: requests.length,
      rows: [...new Map(h.messages.flatMap((m: any) => (m.params?.frame?.payload?.deltas ?? []).filter((d: any) => d.row).map((d: any) => [d.row.rowId, d.row] as const))).values()].map((r: any) => [r.kind, r.text, r.origin, r.guided]),
      messages: scrub(requests.map((r) => r.messages.filter((m: any) => m.role !== "system").slice(-4))),
      hookRows: h.messages.some((m: any) =>
        (m.params?.frame?.payload?.deltas ?? []).some((d: any) => d.row?.kind === "hookInvocation"),
      ),
      schemaErrors: h.schemaErrors,
    };
  } finally {
    await f.close();
  }
}

test("Node and Rust drain the session mailbox through hooks the same way", async () => {
  const node = await observe("node");
  if (process.env.ESCODE_DUMP) console.log(JSON.stringify(node, null, 1));
  const rust = await observe("rust");
  assert.deepEqual(node.schemaErrors, []);
  assert.match(JSON.stringify(node.messages[0]), /first note/);
  for (const key of Object.keys(node)) {
    assert.deepEqual((rust as any)[key], (node as any)[key], key);
  }
});
