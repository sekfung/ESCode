import assert from "node:assert/strict";
import test from "node:test";
import { join, resolve } from "node:path";
import { mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { fixture, event, end } from "./zcode-cli-rust-fixture.js";
import { configureRegistry } from "./zcode-cli-rust-registry-fixture.js";

// docs/specs/rust-dynamic-workflow.md 第 1 期：动态工作流灰度门。
// `workspace/updateDynamicWorkflowPolicy` 的回显与灰度关闭时的工具面在 Node 与 Rust 上一致；
// Rust 在工具实现前不注册工作流工具，所以两侧都不含这份名单（开关只决定之后各期的可见性）。
process.env.ZCODE_TEST_WAIT_MS ??= "30000";
const nodeBundle = resolve("apps/zcode-cli/packages/cli/dist/zcode.cjs");
/** TS `DYNAMIC_WORKFLOW_TOOL_NAMES`（关闭态下架的十个工具；旧的 `Workflow` 工具不在列）。 */
const WORKFLOW_TOOLS = [
  "CreateWorkflow",
  "AmendWorkflow",
  "SaveWorkflow",
  "ListSavedWorkflows",
  "ListModels",
  "EvalWorkflowSnippet",
  "ListWorkflowRuns",
  "GetWorkflowRun",
  "ResumeWorkflowRun",
  "ResolveWorkflowQuestion",
];

async function observe(kind: "node" | "rust") {
  const root = await mkdtemp(join(tmpdir(), `zcode-workflow-policy-${kind}-`));
  const requests: any[] = [];
  const respond = (req: any, res: any) => {
    requests.push(req);
    res.writeHead(200, { "content-type": "text/event-stream" });
    event(res, { content: "done" });
    end(res, "stop");
  };
  const f =
    kind === "node"
      ? await fixture({
          root,
          command: process.execPath,
          args: ({ cwd }) => [
            nodeBundle,
            "app-server",
            "--stdio",
            "--cwd",
            cwd,
          ],
          registry: true,
          respond,
          mode: "yolo",
        })
      : await fixture({ root, registry: true, respond, mode: "yolo" });
  try {
    await configureRegistry(f, false);
    const h = f.start();
    const ask = async (enabled: boolean): Promise<any> =>
      h.client.request(
        "workspace/updateDynamicWorkflowPolicy",
        {
          workspace: { workspacePath: h.workspace, workspaceKey: h.workspace },
          enabled,
        },
        { parse: (value: unknown) => value } as any,
      );
    // 缺省（Host 未调用）即关闭：显式关闭与缺省走同一条读法。
    const enabled = await ask(true);
    const disabled = await ask(false);
    const id = await h.create();
    await h.subscribe(`conversation/${id}`);
    await h.command(
      h.envelope("sendText", id, { text: "hello", mode: "yolo" }),
    );
    await h.completed(id);
    await h.close();
    const names = (requests[0]?.tools ?? [])
      .map((tool: any) => tool.function.name)
      .filter((name: string) => !name.startsWith("mcp__"));
    return { enabled, disabled, names, workspace: h.workspace };
  } finally {
    await f.close();
  }
}

test("dynamic workflow policy and tool surface match Node", async () => {
  const node = await observe("node");
  const rust = await observe("rust");
  // 自检：两侧回显各自的入参，且灰度关闭时下架全部工作流工具。
  assert.equal(node.enabled.enabled, true);
  assert.equal(node.disabled.enabled, false);
  assert.equal(rust.enabled.enabled, true);
  assert.equal(rust.disabled.enabled, false);
  // 回显就是收到的 workspace 对象（TS `params.workspace`，Rust 同）。
  for (const observed of [node, rust]) {
    assert.equal(observed.enabled.workspace.workspacePath, observed.workspace);
    assert.equal(observed.enabled.workspace.workspaceKey, observed.workspace);
  }
  for (const name of WORKFLOW_TOOLS) {
    assert.ok(
      !node.names.includes(name),
      `${name} 应在关闭态下架：${JSON.stringify(node.names)}`,
    );
    assert.ok(
      !rust.names.includes(name),
      `${name} 不应在 Rust 侧出现：${JSON.stringify(rust.names)}`,
    );
  }
  // 模型看到的工具名在关闭态逐字一致（Rust 不注册未实现的工作流工具）。
  assert.deepEqual(rust.names, node.names);
});
