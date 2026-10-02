import assert from "node:assert/strict";
import test from "node:test";
import { dirname, join, resolve } from "node:path";
import { mkdir, mkdtemp, readFile, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { fixture, event, end, type Harness } from "./zcode-cli-rust-fixture.js";
import { configureRegistry } from "./zcode-cli-rust-registry-fixture.js";

// docs/specs/rust-dynamic-workflow.md 第 2/3 期：SaveWorkflow。技能门、入参校验、编译诊断（不弹窗不落盘）、
// 干净脚本的确认（alwaysAsk、无「总是允许」）与落盘、覆盖判定、script_path 读草稿（丢掉元数据块）。
// 诊断来自同一个 TS 分析器（Rust 经 Node 分析子进程），模型看到的结果与确认窗载荷两侧逐字一致。
process.env.ZCODE_TEST_WAIT_MS ??= "60000";
type Message = Record<string, any>;
const nodeBundle = resolve("apps/zcode-cli/packages/cli/dist/zcode.cjs");
const corpus = JSON.parse(
  await readFile(
    resolve("apps/zcode-cli-rust/crates/tools/src/workflow_analysis_corpus.json"),
    "utf8",
  ),
);
const validScript: string = corpus.cases.find((c: any) => c.result.ok && c.script.trim()).script;
const badScript = 'const x: number = "s";\n';

const call = (id: string, name: string, args: unknown) => ({
  tool_calls: [
    { index: 0, id, type: "function", function: { name, arguments: JSON.stringify(args) } },
  ],
});
const save = (id: string, args: Record<string, unknown>) =>
  call(id, "SaveWorkflow", { description: "Review a change", scope: "project", ...args });
const steps = [
  save("sw-gate", { name: "early", script: validScript }),
  call("skill", "Skill", { skill: "dynamic-workflows" }),
  save("sw-name", { name: "bad name", script: validScript }),
  save("sw-both", { name: "both", script: validScript, script_path: "draft.dwf.ts" }),
  save("sw-sentinel", { name: "sentinel", script: `/* zcode-workflow\n*/\n${validScript}` }),
  save("sw-diag", { name: "broken", script: badScript }),
  save("sw-new", { name: "rev", script: validScript, whenToUse: "Before merging" }),
  save("sw-over", { name: "rev", script: validScript }),
  save("sw-path", { name: "glob", script_path: "draft.dwf.ts", scope: "global" }),
];

async function observe(kind: "node" | "rust") {
  const root = await mkdtemp(join(tmpdir(), `zcode-save-workflow-${kind}-`));
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
    await writeFile(
      join(f.cwd, "draft.dwf.ts"),
      `/* zcode-workflow\ndescription: An old draft\n*/\n${validScript}`,
    );
    const h: Harness = f.start();
    const workspace = { workspacePath: h.workspace, workspaceKey: h.workspace };
    await h.client.request("workspace/updateDynamicWorkflowPolicy", { workspace, enabled: true }, {
      parse: (value: unknown) => value,
    } as any);
    const id = await h.create();
    await h.subscribe(`conversation/${id}`);
    const permissions: any[] = [];
    let seen = 0;
    const answered = new Set<string>();
    const pump = setInterval(() => {
      for (; seen < h.messages.length; seen++) {
        const deltas = h.messages[seen]?.params?.frame?.payload?.deltas ?? [];
        for (const delta of deltas) {
          for (const p of delta.patch?.pendingInteractions ?? []) {
            if (p.kind !== "permission" || answered.has(p.interactionId)) continue;
            answered.add(p.interactionId);
            permissions.push({
              toolName: p.payload.toolName,
              detail: p.payload.detail,
              kinds: p.payload.options.map((o: Message) => o.kind),
            });
            const once = p.payload.options.find((o: Message) => /allow.?once/i.test(o.kind));
            void h.command(
              h.envelope("resolveInteraction", id, {
                interactionId: p.interactionId,
                answer: { optionId: once.optionId },
              }),
            );
          }
        }
      }
    }, 20);
    await h.command(h.envelope("sendText", id, { text: "save it", mode: "yolo" }));
    await h.completed(id);
    clearInterval(pump);
    await h.close();
    const rootText = JSON.stringify(root).slice(1, -1);
    const scrub = (value: unknown) =>
      JSON.parse(JSON.stringify(value).replaceAll(rootText, "<root>").replaceAll(root, "<root>"));
    const last = requests.at(-1);
    const results = Object.fromEntries(
      (last?.messages ?? [])
        .filter((m: any) => m.role === "tool" && String(m.tool_call_id).startsWith("sw-"))
        .map((m: any) => [m.tool_call_id, m.content]),
    );
    const read = (path: string) => readFile(path, "utf8").catch(() => null);
    return scrub({
      results,
      permissions,
      projectFile: await read(join(f.cwd, ".zcode", "workflows", "rev.dwf.ts")),
      brokenFile: await read(join(f.cwd, ".zcode", "workflows", "broken.dwf.ts")),
      globalFile: await read(join(root, ".zcode", "workflows", "glob.dwf.ts")),
      schemaErrors: h.schemaErrors,
    });
  } finally {
    await f.close();
  }
}

test("Node and Rust save workflows the same way", async () => {
  const node = await observe("node");
  const rust = await observe("rust");
  for (const key of Object.keys(node)) {
    assert.deepEqual(rust[key], node[key], key);
  }
  assert.deepEqual(node.schemaErrors, []);
  // 自检：三次确认（新建、覆盖、草稿），编译失败与各类拒绝都不弹窗。
  assert.equal(node.permissions.length, 3);
  assert.match(node.results["sw-gate"], /needs the `dynamic-workflows` skill/);
  assert.match(node.results["sw-diag"], /The workflow script has errors/);
  assert.match(node.results["sw-over"], /Replaced the saved workflow 'rev'/);
  assert.equal(node.brokenFile, null);
  assert.ok(String(node.globalFile).includes("description: Review a change"));
});
