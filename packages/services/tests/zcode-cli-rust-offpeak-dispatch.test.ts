import assert from "node:assert/strict";
import test from "node:test";
import { join, resolve } from "node:path";
import { mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { fixture, event, end } from "./zcode-cli-rust-fixture.js";
import { configureRegistry } from "./zcode-cli-rust-registry-fixture.js";

// docs/specs/rust-offpeak.md 第二期：闲时派发轮的工具面、OffPeakCreate/SendMessage 拒绝与 Bash 后台限制
// 在 Node 与 Rust 上一致（模型看到的工具名与每个工具结果逐字比较）。
process.env.ZCODE_TEST_WAIT_MS ??= "30000";
const nodeBundle = resolve("apps/zcode-cli/packages/cli/dist/zcode.cjs");
const text = (m: any) => (typeof m.content === "string" ? m.content : JSON.stringify(m.content));

type Scenario = "dispatch" | "sentinel";
const payloads: Record<Scenario, Record<string, unknown>> = {
  dispatch: { offPeakTaskId: "offpeak-7", offPeakRunType: "init" },
  sentinel: { toolDisallowlist: ["OffPeakCreate"] },
};
const calls: Record<Scenario, { name: string; arguments: unknown }[]> = {
  dispatch: [
    { name: "OffPeakCreate", arguments: { title: "t", prompt: "p" } },
    { name: "OffPeakList", arguments: {} },
  ],
  sentinel: [
    {
      name: "SendMessage",
      arguments: { to: "agent_x", summary: "follow up", message: "continue" },
    },
    { name: "Bash", arguments: { command: "echo bg", run_in_background: true } },
    // 超时后不转后台（闲时轮关闭自动后台）：两侧都按超时终止。
    { name: "Bash", arguments: { command: "echo started; sleep 3", timeout: 1000 } },
  ],
};

async function observe(kind: "node" | "rust", scenario: Scenario) {
  const root = await mkdtemp(join(tmpdir(), `zcode-offpeak-dispatch-${kind}-`));
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
    const next = calls[scenario][done];
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
    h.hostHandlers = { "offPeak/list": () => ({ tasks: [] }) };
    await h.client.request(
      "workspace/updateOffPeakToolPolicy",
      { workspace: { workspacePath: h.workspace, workspaceKey: h.workspace }, enabled: true },
      { parse: (v: unknown) => v } as any,
    );
    const id = await h.create();
    await h.subscribe(`conversation/${id}`);
    await h.command(
      h.envelope("sendText", id, { text: "idle run", mode: "yolo", ...payloads[scenario] }),
    );
    await h.completed(id);
    await h.close();
    const names = (requests[0]?.tools ?? [])
      .map((t: any) => t.function.name)
      .filter((n: string) => !n.startsWith("mcp__"));
    const results = (requests.at(-1)?.messages ?? [])
      .filter((m: any) => m.role === "tool")
      .map((m: any) =>
        text(m)
          .replace(/(ID: )[^\s.]+/g, "$1<id>")
          .replace(/((?:saved|written) to: ).+?(?=\.?(?:\n|$| ))/g, "$1<path>"),
      );
    return { names, results };
  } finally {
    await f.close();
  }
}

for (const scenario of ["dispatch", "sentinel"] as const) {
  test(`OffPeak ${scenario} turn restrictions match Node`, async () => {
    const node = await observe("node", scenario);
    const rust = await observe("rust", scenario);
    // 自检：派发轮隐藏 OffPeakCreate 与 SendMessage，OffPeakList 保留。
    if (scenario === "dispatch") {
      assert.ok(!node.names.includes("OffPeakCreate"), JSON.stringify(node.names));
      assert.ok(!node.names.includes("SendMessage"));
      assert.ok(node.names.includes("OffPeakList"));
    } else {
      assert.match(node.results[0] ?? "", /is not allowed while running an idle-time task/);
    }
    assert.equal(node.results.length, calls[scenario].length);
    assert.deepEqual(rust.names, node.names);
    assert.deepEqual(rust.results, node.results);
  });
}
