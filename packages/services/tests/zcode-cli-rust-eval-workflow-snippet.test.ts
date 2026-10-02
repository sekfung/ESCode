import assert from "node:assert/strict";
import test from "node:test";
import { dirname, join, resolve } from "node:path";
import { mkdtemp, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { fixture, event, end, type Harness } from "./zcode-cli-rust-fixture.js";
import { configureRegistry } from "./zcode-cli-rust-registry-fixture.js";

// docs/specs/rust-dynamic-workflow.md 第 6 期：EvalWorkflowSnippet。技能门、来源二选一、编译诊断（内联与
// 文件两种写法）、正常返回与日志、运行时抛错、无返回值。执行面两侧都是 Node 的 snippet 服务（Rust 经
// `__zcode-workflow-snippet` 子进程），模型看到的结果逐字一致（耗时数字抹掉）。
process.env.ZCODE_TEST_WAIT_MS ??= "60000";
const nodeBundle = resolve("apps/zcode-cli/packages/cli/dist/zcode.cjs");

const call = (id: string, name: string, args: unknown) => ({
  tool_calls: [
    { index: 0, id, type: "function", function: { name, arguments: JSON.stringify(args) } },
  ],
});
const steps = [
  call("ev-gate", "EvalWorkflowSnippet", { code: "return 1;" }),
  call("skill", "Skill", { skill: "dynamic-workflows" }),
  call("ev-both", "EvalWorkflowSnippet", { code: "return 1;", path: "snippet.ts" }),
  call("ev-diag", "EvalWorkflowSnippet", { code: 'const x: number = "s";\nreturn x;' }),
  call("ev-file", "EvalWorkflowSnippet", { path: "snippet.ts" }),
  call("ev-ok", "EvalWorkflowSnippet", {
    code: 'log("first"); log("second");\nreturn { total: 1 + 2, tags: ["a", "b"] };',
  }),
  call("ev-throw", "EvalWorkflowSnippet", { code: 'log("before");\nthrow new Error("boom");' }),
  call("ev-void", "EvalWorkflowSnippet", { code: 'log("only a log");', timeoutMs: 30000 }),
];

async function observe(kind: "node" | "rust") {
  const root = await mkdtemp(join(tmpdir(), `zcode-eval-snippet-${kind}-`));
  const requests: any[] = [];
  const respond = (req: any, res: any) => {
    requests.push(req);
    res.writeHead(200, { "content-type": "text/event-stream" });
    const step = steps[requests.length - 1];
    if (step) {
      event(res, step);
      end(res, "tool_calls");
    } else {
      event(res, { content: "done" });
      end(res, "stop");
    }
  };
  const env = {
    ZCODE_OFFICIAL_PLUGINS_BASE_DIR: dirname(nodeBundle),
    ZCODE_PLUGIN_HOST_EXEC_PATH: process.execPath,
    ZCODE_PLUGIN_HOST_ENTRYPOINT: nodeBundle,
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
          env,
        })
      : await fixture({ root, registry: true, respond, mode: "yolo", env });
  try {
    await configureRegistry(f, false);
    await writeFile(join(f.cwd, "snippet.ts"), 'const y: string = 2;\nreturn y;\n');
    const h: Harness = f.start();
    const workspace = { workspacePath: h.workspace, workspaceKey: h.workspace };
    await h.client.request("workspace/updateDynamicWorkflowPolicy", { workspace, enabled: true }, {
      parse: (value: unknown) => value,
    } as any);
    const id = await h.create();
    await h.subscribe(`conversation/${id}`);
    await h.command(h.envelope("sendText", id, { text: "try it", mode: "yolo" }));
    await h.completed(id);
    await h.close();
    const last = requests.at(-1);
    const results = Object.fromEntries(
      (last?.messages ?? [])
        .filter((m: any) => m.role === "tool" && String(m.tool_call_id).startsWith("ev-"))
        .map((m: any) => [
          m.tool_call_id,
          String(m.content).replace(/completed in \d+ms/, "completed in <n>ms"),
        ]),
    );
    return { results, schemaErrors: h.schemaErrors };
  } finally {
    await f.close();
  }
}

test("Node and Rust evaluate workflow snippets the same way", async () => {
  const node = await observe("node");
  const rust = await observe("rust");
  assert.deepEqual(rust, node);
  assert.deepEqual(node.schemaErrors, []);
  assert.equal(Object.keys(node.results).length, 7);
  assert.match(node.results["ev-gate"], /needs the `dynamic-workflows` skill/);
  assert.match(node.results["ev-diag"], /The snippet has errors/);
  assert.match(node.results["ev-file"], /snippet\.ts:L1:C7/);
  assert.match(node.results["ev-ok"], /Return value:/);
  assert.match(node.results["ev-throw"], /The snippet failed/);
  assert.match(node.results["ev-void"], /It returned no value/);
});
