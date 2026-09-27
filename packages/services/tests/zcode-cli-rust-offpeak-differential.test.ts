import assert from "node:assert/strict";
import test from "node:test";
import { join, resolve } from "node:path";
import { mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { fixture, event, end } from "./zcode-cli-rust-fixture.js";
import { configureRegistry } from "./zcode-cli-rust-registry-fixture.js";

// docs/specs/rust-offpeak.md 第一期：工具面开关（进程级 workspace 结论与会话级创建参数）、
// OffPeakCreate/OffPeakList 往返时 Host 收到的请求与模型看到的定义、结果在 Node 与 Rust 上一致。
process.env.ZCODE_TEST_WAIT_MS ??= "30000";
const nodeBundle = resolve("apps/zcode-cli/packages/cli/dist/zcode.cjs");
const text = (m: any) => (typeof m.content === "string" ? m.content : JSON.stringify(m.content));
const task = {
  offPeakTaskId: "offpeak-1",
  title: "Refactor utils",
  status: "queued",
  queuePosition: 2,
  sessionId: "",
  createdAt: 1767000000000,
};
const calls = [
  {
    name: "OffPeakCreate",
    arguments: { title: " Refactor utils ", prompt: "Refactor the utils directory" },
  },
  { name: "OffPeakList", arguments: {} },
];

type Scenario = "policy" | "none" | "session";

async function observe(kind: "node" | "rust", scenario: Scenario) {
  const root = await mkdtemp(join(tmpdir(), `zcode-offpeak-${kind}-`));
  const requests: any[] = [];
  const respond = (req: any, res: any) => {
    if (
      req.stream === false ||
      req.stream === undefined ||
      text(req.messages[0] ?? {}).startsWith("Generate a concise title")
    ) {
      res.writeHead(200, { "content-type": "text/event-stream" });
      event(res, { content: "Title" });
      end(res, "stop");
      return;
    }
    requests.push(req);
    res.writeHead(200, { "content-type": "text/event-stream" });
    const done = req.messages.filter((m: any) => m.role === "tool").length;
    const next = scenario === "none" ? undefined : calls[done];
    if (next) {
      event(res, {
        tool_calls: [
          {
            index: 0,
            id: `call-${done}`,
            type: "function",
            function: { name: next.name, arguments: JSON.stringify(next.arguments) },
          },
        ],
      });
      end(res, "tool_calls");
      return;
    }
    event(res, { content: "done" });
    end(res, "stop");
  };
  const f =
    kind === "node"
      ? await fixture({
          root,
          command: process.execPath,
          args: ({ cwd }) => [nodeBundle, "app-server", "--stdio", "--cwd", cwd],
          registry: true,
          respond,
          mode: "yolo",
        })
      : await fixture({ root, registry: true, respond, mode: "yolo" });
  try {
    await configureRegistry(f, false);
    const h = f.start();
    const host: any[] = [];
    let sessionId = "";
    h.hostHandlers = {
      "offPeak/list": (params: any) => {
        host.push({ method: "offPeak/list", params });
        return { tasks: [] };
      },
      "offPeak/create": (params: any) => {
        host.push({ method: "offPeak/create", params });
        return { ok: true, task: { ...task, sessionId } };
      },
    };
    let policy: unknown;
    if (scenario === "policy") {
      policy = await h.client.request(
        "workspace/updateOffPeakToolPolicy",
        { workspace: { workspacePath: h.workspace, workspaceKey: h.workspace }, enabled: true },
        { parse: (v: unknown) => v } as any,
      );
    }
    if (scenario === "session") {
      const ack = await h.command(
        h.envelope("createSession", null, { workspaceId: f.cwd, offPeakToolEnabled: true }),
      );
      sessionId = (ack.result as any).sessionId;
    } else {
      sessionId = await h.create();
    }
    await h.subscribe(`conversation/${sessionId}`);
    await h.command(
      h.envelope("sendText", sessionId, { text: "queue this for idle time", mode: "yolo" }),
    );
    await h.completed(sessionId);
    await h.close();
    const first = requests[0];
    // node_repl 只在 Node 的 bundle 中随官方插件出现，与 OffPeak 无关。
    const names = (first?.tools ?? [])
      .map((t: any) => t.function.name)
      .filter((n: string) => !n.startsWith("mcp__"));
    const definitions = (first?.tools ?? []).filter((t: any) =>
      t.function.name.startsWith("OffPeak"),
    );
    const results = requests
      .flatMap((r: any) => r.messages)
      .filter((m: any) => m.role === "tool")
      .map((m: any) => text(m).split(sessionId).join("<sid>"));
    return {
      policy: JSON.parse(
        JSON.stringify(policy ?? null)
          .split(JSON.stringify(h.workspace).slice(1, -1))
          .join("<ws>"),
      ),
      names,
      definitions,
      results: [...new Set(results)],
      host: JSON.parse(JSON.stringify(host).split(sessionId).join("<sid>")),
    };
  } finally {
    await f.close();
  }
}

for (const scenario of ["policy", "none", "session"] as const) {
  test(`OffPeak tool surface and round trip match Node (${scenario})`, async () => {
    const node = await observe("node", scenario);
    const rust = await observe("rust", scenario);
    // 自检：Node 按开关暴露 OffPeak 工具。
    assert.equal(
      node.names.includes("OffPeakCreate"),
      scenario !== "none",
      JSON.stringify(node.names),
    );
    if (scenario !== "none") {
      assert.deepEqual(
        node.host.map((r: any) => r.method),
        ["offPeak/list", "offPeak/create", "offPeak/list"],
      );
      assert.match(node.results.join("\n"), /Created idle-time task offpeak-1 \(#2 in queue\)/);
    }
    assert.deepEqual(rust.policy, node.policy);
    assert.deepEqual(rust.names, node.names);
    assert.deepEqual(rust.definitions, node.definitions);
    assert.deepEqual(rust.host, node.host);
    assert.deepEqual(rust.results, node.results);
  });
}
