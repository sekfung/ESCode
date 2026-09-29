import assert from "node:assert/strict";
import test from "node:test";
import { mkdir, mkdtemp, readFile, writeFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import { tmpdir } from "node:os";
import { event, end, fixture } from "./zcode-cli-rust-fixture.js";
import { configureRegistry } from "./zcode-cli-rust-registry-fixture.js";
import { zcodeSessionSubagentsResultSchema } from "@zcode/shared";

// docs/specs/rust-permission-modes.md「子代理的执行模式」：用户级/插件 profile 的
// `permissionMode: auto|plan` 覆盖继承值；项目级来源在解析时剥离（仓库内容不能提权）。
// 差分口径：子会话里写文件的工具行收口、模型看到的拒绝文案、父会话收到的 Agent 结果、
// 子会话 mode 与磁盘副作用。只比「文件没写成」会漏掉权限模式本身，所以要看拒绝原文。
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

async function observe(kind: Runtime, source: "user" | "project", permissionMode: string) {
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
          args: {
            description: "write one",
            prompt: "child work",
            subagent_type: "locked-agent",
          },
        },
      ]);
    if (isChild && last.role === "user")
      return call(response, [
        { id: "write-1", name: "Write", args: { file_path: "child.txt", content: "written" } },
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
  // 用户级 profile 在 `<storageRoot>/agents`（fixture 的 HOME=root），项目级在 `<cwd>/.zcode/agents`。
  const base = source === "user" ? f.root : f.cwd;
  await mkdir(join(base, ".zcode", "agents"), { recursive: true });
  await writeFile(
    join(base, ".zcode", "agents", "locked-agent.md"),
    `---\nname: locked-agent\ndescription: writes nothing\npermissionMode: ${permissionMode}\n---\nBe careful.\n`,
  );
  try {
    await configureRegistry(f);
    const h = f.start();
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
    assert.ok(childId, "the profile must launch a child session");
    // 单会话按需激活：先订阅子会话，rowsRange 才可读。
    await h.subscribe(`conversation/${childId}`);
    const childRows = (await h.rows(childId)).rows as any[];
    const writes = childRows.filter((r) => r.kind === "toolCall" && r.toolName === "Write");
    if (process.env.ZCODE_SUBAGENT_MODE_DUMP) console.error("ROW_DUMP", kind, JSON.stringify(writes));
    const childRequests = f.requests.filter((r: any) =>
      r.messages.some((m: any) => m.role === "user" && m.content === "child work"),
    );
    const childToolResult = childRequests
      .at(-1)
      ?.messages.filter((m: any) => m.role === "tool")
      .at(-1)?.content;
    // 只比权限收口本身：行状态 + 模型可见正文。行的 output/error 细节（策略拒绝的
    // 投影差异，见 rust-permission-modes.md 的未决项）属于单独一条链路。
    const observation = {
      writeRows: writes.map((r) => r.status),
      childToolResult,
      parentToolResults: f.requests
        .at(-1)
        ?.messages.filter((m: any) => m.role === "tool")
        // agentId 与 duration_ms 是每次运行的噪声，其余正文必须逐字一致。
        .map((m: any) =>
          String(m.content)
            .replace(/agent_[0-9a-f-]+/gu, "agent_UUID")
            .replace(/duration_ms: \d+/gu, "duration_ms: N"),
        ),
      file: await readFile(join(f.cwd, "child.txt"), "utf8").catch(() => undefined),
      schemaErrors: h.schemaErrors,
    };
    await h.close();
    return observation;
  } finally {
    await f.close();
  }
}

for (const permissionMode of ["plan", "auto"] as const) {
  test(`Node and Rust apply a user agent profile with permissionMode: ${permissionMode}`, async () => {
    const node = await observe("node", "user", permissionMode);
    const rust = await observe("rust", "user", permissionMode);
    assert.deepEqual(rust, node);
    // 语义锚点：拒绝必须来自权限判定，而不是「工具没跑」。
    assert.deepEqual(node.writeRows, ["cancelled"]);
    assert.equal(node.file, undefined);
    assert.match(
      String(node.childToolResult),
      permissionMode === "plan"
        ? /Plan mode only allows read-only, non-destructive tools/
        : /Auto mode is reserved but not implemented yet/,
    );
  });
}

test("Node and Rust strip permissionMode declared by a project agent profile", async () => {
  const node = await observe("node", "project", "plan");
  const rust = await observe("rust", "project", "plan");
  assert.deepEqual(rust, node);
  assert.deepEqual(node.writeRows, ["success"]);
  assert.equal(node.file, "written");
});
