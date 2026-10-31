// docs/specs/rust-tool-search.md §5 端到端覆盖：真实 stdio MCP server + 模型 fixture 的 Rust 端到端。
// 本 fork 的 Node CLI 把 toolSearchMode 固定为 off（server-operations.ts），没有 Node 实现可做端到端差分；
// 搜索算法本身仍由 escode-cli-rust-tool-search.test.ts 与上游 oracle 逐条比对。
import assert from "node:assert/strict";
import test from "node:test";
import { readFile } from "node:fs/promises";
import { join } from "node:path";
import { DatabaseSync } from "node:sqlite";
import { fixture, event, end, type Harness, type Message } from "./escode-cli-rust-fixture.js";
import { escodeSessionStateSnapshotSchema } from "@escode/shared";
import { stdioServer } from "./escode-cli-rust-mcp-fixture.js";

const MCP_TOOL = "mcp__local_echo__echo";
const ANNOUNCEMENT = "The following deferred tools are available via ToolSearch.";
const announcementCount = (request: Message) =>
  JSON.stringify(request.messages).split(ANNOUNCEMENT).length - 1;
const names = (request: Message) =>
  (request.tools as Array<{ function: { name: string } }>).map((tool) => tool.function.name);
const call = (id: string, name: string, input: unknown) => ({
  index: 0,
  id,
  type: "function",
  function: { name, arguments: JSON.stringify(input) },
});
const toolMessage = (requests: any[], index: number, id: string) =>
  requests[index]!.messages.find((m: any) => m.role === "tool" && m.tool_call_id === id);

async function send(h: Harness, sid: string, text: string) {
  const after = h.messages.length;
  await h.command(h.envelope("sendText", sid, { text }));
  await h.completed(sid, after);
}

test("local ToolSearch hides MCP tools, loads on search, executes via DeferredToolCall and expands next turn", async () => {
  const f = await fixture({
    mode: "yolo",
    env: { ESCODE_TOOL_SEARCH_MODE: "local" },
    respond(req, res) {
      res.writeHead(200, { "content-type": "text/event-stream" });
      const last = req.messages.at(-1);
      if (last.role === "user") {
        if (String(last.content) === "search then call")
          event(res, { tool_calls: [call("search-1", "ToolSearch", { query: "echo" })] });
        else if (String(last.content) === "search again")
          event(res, { tool_calls: [call("search-again", "ToolSearch", { query: "echo" })] });
        else event(res, { tool_calls: [call("direct-1", MCP_TOOL, { text: "direct" })] });
        end(res, "tool_calls");
      } else if (last.role === "tool" && last.tool_call_id === "search-1") {
        event(res, {
          tool_calls: [
            call("defer-1", "DeferredToolCall", { name: MCP_TOOL, input: { text: "via search" } }),
          ],
        });
        end(res, "tool_calls");
      } else {
        event(res, { content: "done" });
        end(res, "stop");
      }
    },
  });
  const server = await stdioServer(f.root);
  try {
    const h = f.start();
    const ack = await h.command(
      h.envelope("createSession", null, { workspaceId: f.cwd, mcpServers: [server.config] }),
    );
    const mcpSession = (ack.result as { sessionId: string }).sessionId;
    await h.subscribe(`conversation/${mcpSession}`);

    await send(h, mcpSession, "search then call");

    // 首轮：MCP 工具延迟，控制工具可见。
    assert.ok(names(f.requests[0]!).includes("ToolSearch"));
    assert.ok(names(f.requests[0]!).includes("DeferredToolCall"));
    assert.ok(!names(f.requests[0]!).includes(MCP_TOOL));
    // ToolSearch 命中写回模型：结果内带完整 <functions> schema，同一 run 内工具面不变，
    // 目标工具通过 DeferredToolCall 执行。
    const search = String(toolMessage(f.requests, 1, "search-1")?.content);
    assert.match(search, new RegExp(MCP_TOOL));
    assert.match(search, /<functions>\n<function>\{/);
    assert.match(search, /"parameters":/);
    assert.match(search, /Loaded: mcp__local_echo__echo\. Call it now with DeferredToolCall/);
    assert.ok(!names(f.requests[1]!).includes(MCP_TOOL));
    assert.match(String(toolMessage(f.requests, 2, "defer-1")?.content), /via search/);

    await send(h, mcpSession, "direct call");
    // 下一用户轮：已发现工具按完整 schema 展开。
    assert.ok(names(f.requests[3]!).includes(MCP_TOOL));
    assert.match(String(toolMessage(f.requests, 4, "direct-1")?.content), /direct/);

    await send(h, mcpSession, "search again");
    // 再次搜索已加载工具：不再重复展开定义，只给“会话内已加载”提醒。
    const again = String(toolMessage(f.requests, 6, "search-again")?.content);
    assert.ok(!again.includes("<functions>"));
    assert.match(
      again,
      /Tool definitions already available in this conversation: mcp__local_echo__echo\./,
    );
    assert.match(again, /Searching again does not execute them\./);
    assert.ok(!again.includes("Call it now with DeferredToolCall"));

    const calls = (await readFile(server.calls, "utf8"))
      .trim()
      .split("\n")
      .filter(Boolean)
      .map((line) => JSON.parse(line).arguments.text);
    assert.deepEqual(calls, ["via search", "direct"]);
    assert.deepEqual(h.schemaErrors, []);
    await h.close();
  } finally {
    await f.close();
  }
});

test("announces deferred MCP tools once and does not repeat on the next user turn", async () => {
  const f = await fixture({
    mode: "yolo",
    env: { ESCODE_TOOL_SEARCH_MODE: "local" },
    respond(_req, res) {
      res.writeHead(200, { "content-type": "text/event-stream" });
      event(res, { content: "done" });
      end(res, "stop");
    },
  });
  const server = await stdioServer(f.root);
  try {
    const h = f.start();
    const ack = await h.command(
      h.envelope("createSession", null, { workspaceId: f.cwd, mcpServers: [server.config] }),
    );
    const sid = (ack.result as { sessionId: string }).sessionId;
    await h.subscribe(`conversation/${sid}`);

    await send(h, sid, "first turn");
    assert.equal(announcementCount(f.requests[0]!), 1);
    const content = JSON.stringify(f.requests[0]!.messages);
    assert.match(content, /<system-reminder>/);
    assert.match(
      content,
      /Call resident tools directly:\\n(?:mcp__[^"\\n]+\\n)*mcp__local_echo__echo/,
    );

    // 下一用户轮：历史里已有公告增量事实，不再重复通告。
    await send(h, sid, "second turn");
    assert.equal(f.requests.length, 2);
    assert.equal(announcementCount(f.requests[1]!), 1);
    assert.deepEqual(h.schemaErrors, []);
    await h.close();
  } finally {
    await f.close();
  }
});

test("DeferredToolCall before ToolSearch executes the deferred target", async () => {
  const f = await fixture({
    mode: "yolo",
    env: { ESCODE_TOOL_SEARCH_MODE: "local" },
    respond(req, res) {
      res.writeHead(200, { "content-type": "text/event-stream" });
      const last = req.messages.at(-1);
      if (last.role === "user")
        event(res, {
          tool_calls: [
            call("defer-early", "DeferredToolCall", {
              name: MCP_TOOL,
              input: { text: "too soon" },
            }),
          ],
        });
      else event(res, { content: "done" });
      end(res, last.role === "user" ? "tool_calls" : "stop");
    },
  });
  const server = await stdioServer(f.root);
  try {
    const h = f.start();
    const ack = await h.command(
      h.envelope("createSession", null, { workspaceId: f.cwd, mcpServers: [server.config] }),
    );
    const sid = (ack.result as { sessionId: string }).sessionId;
    await h.subscribe(`conversation/${sid}`);
    await send(h, sid, "call without searching");
    const result = toolMessage(f.requests, 1, "defer-early");
    // 未搜索不是准入条件（上游 resolveDeferredToolCall 同）：目标按注册表 schema 校验后执行。
    assert.match(String(result?.content), /too soon/);
    const calls = (await readFile(server.calls, "utf8"))
      .trim()
      .split("\n")
      .filter(Boolean)
      .map((line) => JSON.parse(line).arguments.text);
    assert.deepEqual(calls, ["too soon"]);
    assert.deepEqual(h.schemaErrors, []);
    await h.close();
  } finally {
    await f.close();
  }
});

test("invalid undiscovered input returns schema guidance and does not execute", async () => {
  const f = await fixture({
    mode: "yolo",
    env: { ESCODE_TOOL_SEARCH_MODE: "local" },
    respond(req, res) {
      res.writeHead(200, { "content-type": "text/event-stream" });
      const last = req.messages.at(-1);
      if (last.role === "user") {
        event(res, {
          tool_calls: [call("invalid", "DeferredToolCall", { name: MCP_TOOL, input: {} })],
        });
        end(res, "tool_calls");
      } else if (last.role === "tool" && last.tool_call_id === "invalid") {
        event(res, { tool_calls: [call("search", "ToolSearch", { query: "echo" })] });
        end(res, "tool_calls");
      } else if (last.role === "tool" && last.tool_call_id === "search") {
        event(res, {
          tool_calls: [call("known", "DeferredToolCall", { name: MCP_TOOL, input: {} })],
        });
        end(res, "tool_calls");
      } else {
        event(res, { content: "done" });
        end(res, "stop");
      }
    },
  });
  const server = await stdioServer(f.root);
  try {
    const h = f.start();
    const ack = await h.command(
      h.envelope("createSession", null, { workspaceId: f.cwd, mcpServers: [server.config] }),
    );
    const sid = (ack.result as { sessionId: string }).sessionId;
    await h.subscribe(`conversation/${sid}`);
    await send(h, sid, "call with bad input");

    const invalid = String(toolMessage(f.requests, 1, "invalid")?.content);
    assert.match(invalid, /InputValidationError/);
    assert.match(invalid, /This tool's schema was not sent to the API/);
    assert.match(invalid, /call ToolSearch with query "select:mcp__local_echo__echo"/);
    assert.match(
      invalid,
      /For reference, this tool's input schema is: \{"type":"object","properties":\{"text":\{"type":"string"\}\},"required":\["text"\]\}/,
    );
    assert.match(
      invalid,
      /Call DeferredToolCall with \{name: "mcp__local_echo__echo", input: \{\.\.\.\}\}\./,
    );

    // 已发现后同样的非法入参只回普通校验文案，不再给 schema 与加载提示。
    const known = String(toolMessage(f.requests, 3, "known")?.content);
    assert.match(known, /InputValidationError/);
    assert.ok(!known.includes("This tool's schema was not sent to the API"));
    assert.ok(!known.includes("select:mcp__local_echo__echo"));

    // 两次都未执行目标工具。
    assert.equal((await readFile(server.calls, "utf8").catch(() => "")).trim(), "");
    assert.deepEqual(h.schemaErrors, []);
    await h.close();
  } finally {
    await f.close();
  }
});

test("off mode keeps MCP tools resident and injects no control tools", async () => {
  const f = await fixture({
    mode: "yolo",
    respond(req, res) {
      res.writeHead(200, { "content-type": "text/event-stream" });
      const last = req.messages.at(-1);
      if (last.role === "user")
        event(res, { tool_calls: [call("off-call", MCP_TOOL, { text: "off path" })] });
      else event(res, { content: "done" });
      end(res, last.role === "user" ? "tool_calls" : "stop");
    },
  });
  const server = await stdioServer(f.root);
  try {
    const h = f.start();
    const ack = await h.command(
      h.envelope("createSession", null, { workspaceId: f.cwd, mcpServers: [server.config] }),
    );
    const sid = (ack.result as { sessionId: string }).sessionId;
    await h.subscribe(`conversation/${sid}`);
    await send(h, sid, "direct off call");
    assert.ok(names(f.requests[0]!).includes(MCP_TOOL));
    assert.ok(!names(f.requests[0]!).includes("ToolSearch"));
    assert.equal(announcementCount(f.requests[0]!), 0);
    assert.match(String(toolMessage(f.requests, 1, "off-call")?.content), /off path/);
    assert.deepEqual(h.schemaErrors, []);
    await h.close();
  } finally {
    await f.close();
  }
});

test("keeps a tool discovered before compaction resident after a cold restart", async () => {
  const f = await fixture({
    mode: "yolo",
    env: { ESCODE_TOOL_SEARCH_MODE: "local" },
    respond(req, res) {
      res.writeHead(200, { "content-type": "text/event-stream" });
      const last = req.messages.at(-1);
      if (last.role === "user" && String(last.content) === "load tool") {
        event(res, {
          tool_calls: [call("search-before-compact", "ToolSearch", { query: "echo" })],
        });
        end(res, "tool_calls");
      } else if (last.role === "user" && String(last.content) === "after compact") {
        event(res, {
          tool_calls: [call("direct-after-compact", MCP_TOOL, { text: "after compact" })],
        });
        end(res, "tool_calls");
      } else {
        event(res, { content: "done" });
        end(res, "stop");
      }
    },
  });
  const server = await stdioServer(f.root);
  try {
    const h = f.start();
    const ack = await h.command(
      h.envelope("createSession", null, { workspaceId: f.cwd, mcpServers: [server.config] }),
    );
    const sid = (ack.result as { sessionId: string }).sessionId;
    await h.subscribe(`conversation/${sid}`);
    await send(h, sid, "load tool");
    assert.ok(!names(f.requests[0]!).includes(MCP_TOOL));
    assert.match(
      String(toolMessage(f.requests, 1, "search-before-compact")?.content),
      new RegExp(MCP_TOOL),
    );

    const after = h.messages.length;
    await h.command(h.envelope("compact", sid));
    await h.completed(sid, after);
    // 压缩边界必须持久化已发现名单（rust-tool-search.md §5.2）：冷进程只能从这里重建。
    const db = new DatabaseSync(join(f.dataDir, "rust-sessions.sqlite"));
    const body = db.prepare("SELECT body FROM rust_session WHERE id=?").get(sid) as {
      body: string;
    };
    db.close();
    assert.deepEqual(JSON.parse(body.body).context.preCompactDiscoveredTools, [MCP_TOOL]);
    await h.close();

    // 冷恢复：压缩边界里的已发现名单必须让该工具继续常驻，且不重新公告。
    const cold = f.start();
    try {
      // App 冷恢复路径：session/resume 重新带上 create 时的 MCP 配置。
      await cold.client.request(
        "session/resume",
        {
          workspace: { workspacePath: cold.workspace, workspaceKey: cold.workspace },
          sessionId: sid,
          mcpServers: [server.config],
        },
        escodeSessionStateSnapshotSchema,
      );
      await cold.subscribe(`conversation/${sid}`);
      const before = f.requests.length;
      await send(cold, sid, "after compact");
      assert.ok(names(f.requests[before]!).includes(MCP_TOOL));
      assert.equal(announcementCount(f.requests[before]!), 0);
      assert.match(
        String(toolMessage(f.requests, before + 1, "direct-after-compact")?.content),
        /after compact/,
      );
      assert.deepEqual(cold.schemaErrors, []);
    } finally {
      await cold.close();
    }
  } finally {
    await f.close();
  }
});

test("keeps a locally discovered tool resident after a cold restart without compaction", async () => {
  // local 模式没有 tool_reference；命中名必须随 ToolSearch 结果消息持久化，
  // 否则未压缩的冷恢复会重新把目标工具当成延迟工具（rust-tool-search.md §5.1）。
  const f = await fixture({
    mode: "yolo",
    env: { ESCODE_TOOL_SEARCH_MODE: "local" },
    respond(req, res) {
      res.writeHead(200, { "content-type": "text/event-stream" });
      const last = req.messages.at(-1);
      if (last.role === "user" && String(last.content) === "load tool") {
        event(res, {
          tool_calls: [call("search-before-restart", "ToolSearch", { query: "echo" })],
        });
        end(res, "tool_calls");
      } else if (last.role === "user" && String(last.content) === "after restart") {
        event(res, {
          tool_calls: [call("direct-after-restart", MCP_TOOL, { text: "after restart" })],
        });
        end(res, "tool_calls");
      } else {
        event(res, { content: "done" });
        end(res, "stop");
      }
    },
  });
  const server = await stdioServer(f.root);
  try {
    const h = f.start();
    const ack = await h.command(
      h.envelope("createSession", null, { workspaceId: f.cwd, mcpServers: [server.config] }),
    );
    const sid = (ack.result as { sessionId: string }).sessionId;
    await h.subscribe(`conversation/${sid}`);
    await send(h, sid, "load tool");
    assert.ok(!names(f.requests[0]!).includes(MCP_TOOL));
    assert.equal(announcementCount(f.requests[0]!), 1);
    assert.match(
      String(toolMessage(f.requests, 1, "search-before-restart")?.content),
      new RegExp(MCP_TOOL),
    );

    // 结果消息要带已发现名单；不依赖压缩边界。
    const db = new DatabaseSync(join(f.dataDir, "rust-sessions.sqlite"));
    const body = db.prepare("SELECT body FROM rust_session WHERE id=?").get(sid) as {
      body: string;
    };
    const stored = db
      .prepare("SELECT body FROM rust_message WHERE session=? ORDER BY ordinal")
      .all(sid) as Array<{ body: string }>;
    db.close();
    const persisted = JSON.parse(body.body);
    assert.equal(persisted.context.preCompactDiscoveredTools, undefined);
    const search = stored
      .map((row) => JSON.parse(row.body))
      .find((m) => m.role === "tool" && m.tool_call_id === "search-before-restart");
    assert.ok(search);
    assert.deepEqual(search._escode_tool_search_discovered, [MCP_TOOL]);
    await h.close();

    // 冷恢复：历史里的已发现名单让该工具继续常驻，且不再生成第二条公告。
    const cold = f.start();
    try {
      await cold.client.request(
        "session/resume",
        {
          workspace: { workspacePath: cold.workspace, workspaceKey: cold.workspace },
          sessionId: sid,
          mcpServers: [server.config],
        },
        escodeSessionStateSnapshotSchema,
      );
      await cold.subscribe(`conversation/${sid}`);
      const before = f.requests.length;
      await send(cold, sid, "after restart");
      assert.ok(names(f.requests[before]!).includes(MCP_TOOL));
      // 历史里的原始公告仍在（1 条）；未压缩的冷恢复不得再生成第二条。
      assert.equal(announcementCount(f.requests[before]!), 1);
      assert.match(
        String(toolMessage(f.requests, before + 1, "direct-after-restart")?.content),
        /after restart/,
      );
      assert.deepEqual(cold.schemaErrors, []);
    } finally {
      await cold.close();
    }
  } finally {
    await f.close();
  }
});
