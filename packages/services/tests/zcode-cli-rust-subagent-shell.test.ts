import assert from "node:assert/strict";
import test from "node:test";
import { resolve } from "node:path";
import { event, end, fixture } from "./zcode-cli-rust-fixture.js";
import { zcodeSessionSubagentsResultSchema } from "@zcode/shared";
import { configureRegistry } from "./zcode-cli-rust-registry-fixture.js";

// docs/specs/rust-shell-selection.md「已知差异」：TS 子代理继承父会话已解析的 shell 选择
// （subagent.ts 把父的 bashShellSelection/envInfo.shell 传给 child），不再以自己的 sessionId
// 请求 `user-execution`；Rust 之前每个子会话都另发一次请求。
// 差分口径：两侧的 runtime 偏好请求（按 parent/child 归一）与子会话 Bash 的实际 shell。
const nodeBundle = resolve("apps/zcode-cli/packages/cli/dist/zcode.cjs");
type Runtime = "node" | "rust";

function call(response: any, calls: { id: string; name: string; args: unknown }[]) {
  event(response, {
    tool_calls: calls.map((c, index) => ({
      index,
      id: c.id,
      type: "function",
      function: { name: c.name, arguments: JSON.stringify(c.args) },
    })),
  });
  end(response, "tool_calls");
}
function text(response: any, content: string) {
  event(response, { content });
  end(response, "stop");
}

async function observe(kind: Runtime) {
  const respond = (request: any, response: any) => {
    response.writeHead(200, { "content-type": "text/event-stream" });
    const last = request.messages.at(-1);
    const isChild = request.messages.some(
      (m: any) => m.role === "user" && m.content === "child work",
    );
    if (!isChild && last.role === "user")
      return call(response, [
        {
          id: "agent-1",
          name: "Agent",
          args: { description: "shell probe", prompt: "child work" },
        },
      ]);
    if (isChild && last.role === "user")
      // `ver` 是 cmd 内建命令（Git Bash 下报 command not found），用来区分实际 shell。
      // 不能用 `echo %OS%`：子进程环境被清洗后该变量为空，两种 shell 都会原样输出。
      return call(response, [
        { id: "bash-1", name: "Bash", args: { command: "ver" } },
      ]);
    return text(response, isChild ? "child done" : "parent done");
  };
  const f =
    kind === "node"
      ? await fixture({
          command: process.execPath,
          args: ({ cwd }) => [nodeBundle, "app-server", "--stdio", "--cwd", cwd],
          registry: true,
          mode: "yolo",
          respond,
        })
      : await fixture({ registry: true, mode: "yolo", respond });
  try {
    await configureRegistry(f);
    const h = f.start();
    h.integratedTerminalShell = {
      mode: "shell",
      dialect: "cmd",
      id: "cmd",
      label: "CMD",
      path: "cmd.exe",
    };
    const sid = await h.create();
    await h.subscribe(`conversation/${sid}`);
    await h.command(h.envelope("sendText", sid, { text: "start" }));
    await h.completed(sid);
    const agents = await h.client.request(
      "session/subagents",
      { sessionId: sid },
      zcodeSessionSubagentsResultSchema,
    );
    const childId = agents.ended.items[0]?.childSessionId;
    assert.ok(childId, "the Agent call must launch a child session");
    const childRequests = f.requests.filter((r: any) =>
      r.messages.some((m: any) => m.role === "user" && m.content === "child work"),
    );
    const observation = {
      // 只比会话归属与 scope，不比每次随机的 requestId。
      preferenceRequests: (h.runtimePreferenceRequests as any[]).map((p) => ({
        session: p.sessionId === sid ? "parent" : p.sessionId === childId ? "child" : "other",
        scope: p.scope,
      })),
      // 子会话模型看到的 Bash 结果：cmd 打印 Windows 版本，Git Bash 报 command not found。
      childBash: childRequests
        .at(-1)
        ?.messages.filter((m: any) => m.role === "tool")
        .at(-1)?.content,
      schemaErrors: h.schemaErrors,
    };
    await h.close();
    return observation;
  } finally {
    await f.close();
  }
}

test(
  "subagents inherit the parent shell selection without their own preference request",
  { skip: process.platform !== "win32" },
  async () => {
    const node = await observe("node");
    const rust = await observe("rust");
    assert.deepEqual(rust, node);
    assert.match(String(node.childBash), /Microsoft Windows/);
    assert.ok(
      node.preferenceRequests.every((p) => p.session !== "child"),
      JSON.stringify(node.preferenceRequests),
    );
  },
);
