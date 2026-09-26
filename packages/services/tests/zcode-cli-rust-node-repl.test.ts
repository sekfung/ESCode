import assert from "node:assert/strict";
import test from "node:test";
import { dirname, resolve } from "node:path";
import { zcodeMcpListResultSchema } from "@zcode/shared";
import { fixture, event, end } from "./zcode-cli-rust-fixture.js";
import { configureRegistry } from "./zcode-cli-rust-registry-fixture.js";

// docs/specs/rust-browser-use.md 第 1 期：Browser Use 启用且 node-repl-host 存在时注册内置 node_repl，
// 经 Host 提供的插件启动器运行同一个真实宿主；两侧的设置页状态、会话工具定义与一次 js 调用结果一致。
const nodeBundle = resolve("apps/zcode-cli/packages/cli/dist/zcode.cjs");
const env = {
  ZCODE_OFFICIAL_PLUGINS_BASE_DIR: dirname(nodeBundle),
  ZCODE_PLUGIN_HOST_EXEC_PATH: process.execPath,
  ZCODE_PLUGIN_HOST_ENTRYPOINT: nodeBundle,
};

async function observe(kind: "node" | "rust") {
  let definitions: any[] = [];
  let toolResult: unknown;
  const respond = (req: any, res: any) => {
    res.writeHead(200, { "content-type": "text/event-stream" });
    definitions = (req.tools ?? []).filter((t: any) =>
      t.function.name.startsWith("mcp__node_repl__"),
    );
    if (req.messages.at(-1).role !== "tool") {
      event(res, {
        tool_calls: [
          {
            index: 0,
            id: "c1",
            type: "function",
            function: {
              name: "mcp__node_repl__js",
              arguments: JSON.stringify({ title: "Multiply", code: "console.log(6 * 7)" }),
            },
          },
        ],
      });
      end(res, "tool_calls");
    } else {
      toolResult = req.messages.at(-1).content;
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
          env,
        })
      : await fixture({ registry: true, respond, mode: "yolo", env });
  try {
    await configureRegistry(f, false);
    const h = f.start();
    const listed = await h.client.request(
      "mcp/list",
      { workspace: { workspacePath: f.cwd, workspaceKey: f.cwd }, mode: "connect" },
      zcodeMcpListResultSchema,
    );
    const status = listed.statuses.node_repl;
    const id = await h.create();
    await h.subscribe(`conversation/${id}`);
    await h.command(h.envelope("sendText", id, { text: "run js" }));
    await h.completed(id);
    await h.close();
    return {
      status: status && {
        status: status.status,
        toolCount: status.toolCount,
        era: status.protocolEra,
      },
      definitions: definitions.map((d) => d.function.name).sort(),
      toolResult,
    };
  } finally {
    await f.close();
  }
}

test("node_repl is registered and runs JavaScript the same way as Node", async () => {
  const node = await observe("node");
  const rust = await observe("rust");
  // 设置页 mcp/list 不列出内置 node_repl；会话里可用并执行。
  assert.equal(node.status, undefined);
  assert.equal(node.toolResult, "42");
  assert.deepEqual(rust, node);
});

// 第 2 期：node_repl 经私有 broker 访问浏览器，runtime 把 list/execute 转成 Host 的
// interaction/browserList / interaction/browserExecute（请求上下文逐项一致）。
const BROWSER_JS = [
  // browser-use 技能的 bootstrap（skills/control-browser/SKILL.md）：每次 js 调用都在新内核中。
  'const { join } = await import("node:path");',
  'const { pathToFileURL } = await import("node:url");',
  'const url = pathToFileURL(join(process.env.ZCODE_PLUGIN_ROOT, "scripts", "browser-client.mjs")).href;',
  "const { setupBrowserRuntime } = await import(url);",
  "await setupBrowserRuntime({ globals: globalThis });",
  "const browsers = await agent.browsers.list();",
  "const browser = await agent.browsers.getDefault();",
  "const tabs = await browser.tabs.list();",
  "console.log(JSON.stringify({ browsers: browsers.map((b) => b.id), tabs: tabs.map((t) => t.url) }));",
].join("\n");

async function observeBrowser(kind: "node" | "rust") {
  let toolResult: unknown;
  const respond = (req: any, res: any) => {
    res.writeHead(200, { "content-type": "text/event-stream" });
    if (req.messages.at(-1).role !== "tool") {
      event(res, {
        tool_calls: [
          {
            index: 0,
            id: "c1",
            type: "function",
            function: {
              name: "mcp__node_repl__js",
              arguments: JSON.stringify({ title: "Tabs", code: BROWSER_JS }),
            },
          },
        ],
      });
      end(res, "tool_calls");
    } else {
      toolResult = req.messages.at(-1).content;
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
          env,
        })
      : await fixture({ registry: true, respond, mode: "yolo", env });
  try {
    await configureRegistry(f, false);
    const h = f.start();
    h.hostHandlers["interaction/browserList"] = () => ({
      browsers: [
        { id: "iab-1", generation: 0, type: "iab", name: "In-app browser", capabilities: {} },
      ],
    });
    h.hostHandlers["interaction/browserExecute"] = (params: any) =>
      params.command.method === "list"
        ? {
            ok: true,
            elapsedMs: 1,
            tabs: [
              {
                tabId: "t1",
                url: "https://example.com/",
                title: "Example",
                viewport: { width: 800, height: 600 },
                active: true,
              },
            ],
          }
        : { ok: true, elapsedMs: 1 };
    const id = await h.create();
    await h.subscribe(`conversation/${id}`);
    await h.command(h.envelope("sendText", id, { text: "list tabs" }));
    await h.completed(id);
    await h.close();
    const browserRequests = h.hostRequests
      .filter((r: any) => r.method.startsWith("interaction/browser"))
      .map((r: any) => ({
        method: r.method,
        keys: Object.keys(r.params).sort(),
        command: r.params.command?.method ?? null,
        session: r.params.sessionId === id,
        turn: typeof r.params.turnId === "string",
        clientMode: r.params.clientMode,
        sessionContext: r.params.sessionContext,
        workspace: r.params.workspaceKey === f.cwd && r.params.workspacePath === f.cwd,
      }));
    return { toolResult, browserRequests };
  } finally {
    await f.close();
  }
}

test("node_repl browser calls go through the runtime broker to the Host the same way as Node", async () => {
  const node = await observeBrowser("node");
  const rust = await observeBrowser("rust");
  assert.match(String(node.toolResult), /iab-1/);
  // Node 的 app-server 路径实测不发 turnEnded（rust-browser-use.md），两侧请求序列逐项一致。
  assert.deepEqual(rust, node);
});
