// Run with node --import tsx. 已保存工作流存储层的 TS oracle（docs/specs/rust-dynamic-workflow.md 第 2 期）：
// 在固定目录树上跑真实 listSavedWorkflows / resolveSavedWorkflow / saveSavedWorkflow /
// savedWorkflowExists / findSavedWorkflowShadowing / moveSavedWorkflow / validateWorkflowArgs。
// 读用例比对结果全文；写用例每个都从一份新树开始，比对返回值与结束后的文件快照。
// OS 错误文案（EISDIR/EACCES）与 YAML 解析器措辞跨平台不同：语料记 kind，Rust 侧只比 kind 与固定文案。
// --check 防漂移。
import { mkdir, mkdtemp, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, relative } from "node:path";
import {
  findSavedWorkflowShadowing,
  listSavedWorkflows,
  moveSavedWorkflow,
  resolveSavedWorkflow,
  saveSavedWorkflow,
  savedWorkflowExists,
  serializeSavedWorkflow,
} from "../apps/escode-cli/packages/core/src/tool/handlers/saved-workflows/index.ts";
import { validateWorkflowArgs } from "../apps/escode-cli/packages/core/src/tool/handlers/saved-workflows/args.ts";

const check = process.argv.includes("--check");
const target = new URL(
  "../apps/escode-cli-rust/crates/tools/tests/fixtures/saved_workflow_store_corpus.json",
  import.meta.url,
);

const script = "export default async () => {\n  return 1;\n}\n";
const saved = (meta) => serializeSavedWorkflow(meta, script);

const tree = {
  "home/.escode/workflows/global-only.dwf.ts": saved({ description: "Global only" }),
  "home/.escode/workflows/shared.dwf.ts": saved({
    description: "Global shared",
    whenToUse: "From the global scope",
  }),
  "home/.escode/workflows/broken.dwf.ts": "/* escode-workflow\ndescription: [unclosed\n*/\n",
  "home/.escode/workflows/notes.txt": "not a workflow\n",
  "repo/.escode/workflows/shared.dwf.ts": saved({ description: "Project shared" }),
  "repo/.escode/workflows/proj-only.dwf.ts": saved({
    description: "Project only",
    whenToUse: "Only here",
    args: { pr: { type: "string", required: true }, n: { type: "number", default: 3 } },
  }),
  "repo/.escode/workflows/.dwf.ts": saved({ description: "Dots only" }),
  "repo/.escode/workflows/bad name.dwf.ts": saved({ description: "Space in name" }),
  // null = 目录：目录项存在却读不出来，是"坏了"而不是"没有"。
  "repo/.escode/workflows/dir.dwf.ts": null,
  "repo/README.md": "repo\n",
};
const bareTree = { "repo/README.md": "repo\n" };

async function materialize(root, files) {
  for (const [path, content] of Object.entries(files)) {
    const full = join(root, path);
    if (content === null) await mkdir(full, { recursive: true });
    else {
      await mkdir(dirname(full), { recursive: true });
      await writeFile(full, content);
    }
  }
}

async function snapshot(root) {
  const out = {};
  const walk = async (dir) => {
    for (const entry of await readdir(dir, { withFileTypes: true })) {
      const full = join(dir, entry.name);
      if (entry.isDirectory()) await walk(full);
      else out[relative(root, full).split(/[\\/]/).join("/")] = await readFile(full, "utf8");
    }
  };
  await walk(root);
  return out;
}

/** 目录不参与快照（空目录跨平台不可靠）；文件正文逐字节记录。 */
async function run(tree, body) {
  const root = await mkdtemp(join(tmpdir(), "escode-saved-workflows-"));
  try {
    await materialize(root, tree);
    const rel = (path) => relative(root, path).split(/[\\/]/).join("/");
    return await body({ root, home: join(root, "home"), cwd: join(root, "repo"), rel });
  } finally {
    await rm(root, { recursive: true, force: true });
  }
}

const invalidKind = (reason) => {
  if (reason === "file name is not a usable workflow name") return "not_a_workflow_name";
  if (/^(missing_frontmatter|unterminated_frontmatter|invalid_yaml|invalid_metadata): /.test(reason))
    return "parse_error";
  return "read_error";
};
const entry = (value, rel) => ({
  name: value.name,
  description: value.description,
  ...(value.whenToUse === undefined ? {} : { whenToUse: value.whenToUse }),
  ...(value.args === undefined ? {} : { args: value.args }),
  scope: value.scope,
  path: rel(value.path),
});

// ---- 读用例（同一棵树上按序跑，互不改动文件）----
const reads = await run(tree, ({ home, cwd, rel }) => {
  const cases = [];
  const listed = (result) => ({
    entries: result.entries.map((value) => entry(value, rel)),
    invalid: result.invalid.map((value) => {
      const kind = invalidKind(value.reason);
      return {
        path: rel(value.path),
        kind,
        ...(kind === "not_a_workflow_name" ? { reason: value.reason } : {}),
      };
    }),
  });
  for (const scope of [undefined, "project", "global"]) {
    cases.push({
      op: "list",
      scope: scope ?? null,
      result: listed(listSavedWorkflows({ cwd, homeDir: home, scope })),
    });
  }
  const resolved = (result) => {
    if (result.ok)
      return {
        ok: true,
        name: result.name,
        scope: result.scope,
        path: rel(result.path),
        meta: result.meta,
        script: result.script,
        source: result.source,
        bodyLineOffset: result.bodyLineOffset,
      };
    const failure = { ok: false, reason: result.reason };
    if (result.reason === "invalid_name") failure.detail = result.detail;
    if (result.reason === "parse_error") failure.detail = result.detail;
    if (result.reason === "parse_error" || result.reason === "read_error")
      failure.path = rel(result.path);
    return failure;
  };
  const resolves = [
    { name: "shared" },
    { name: "global-only" },
    { name: "missing" },
    { name: "bad/name" },
    { name: "broken" },
    { name: "broken", scope: "project" },
    { name: "shared", scope: "global" },
    { name: "dir" },
  ];
  for (const input of resolves) {
    cases.push({
      op: "resolve",
      input,
      result: resolved(resolveSavedWorkflow({ cwd, homeDir: home, ...input })),
    });
  }
  for (const input of [
    { name: "shared" },
    { name: "shared", scope: "global" },
    { name: "bad name" },
    { name: "missing" },
  ]) {
    cases.push({
      op: "exists",
      input,
      result: savedWorkflowExists({ cwd, homeDir: home, ...input }),
    });
  }
  for (const input of [
    { name: "shared", scope: "project" },
    { name: "shared", scope: "global" },
    { name: "global-only", scope: "project" },
    { name: "bad name", scope: "project" },
  ]) {
    cases.push({
      op: "shadowing",
      input,
      result: findSavedWorkflowShadowing({ cwd, homeDir: home, ...input }) ?? null,
    });
  }
  return cases;
});

// ---- 写用例（每个都从一份新树开始）----
const mutations = [];
const saveCases = [
  { tree, name: "fresh", scope: "project", meta: { description: "Fresh", whenToUse: "Now" } },
  { tree, name: "shared", scope: "project", meta: { description: "Replace shared" } },
  { tree, name: "glob-new", scope: "global", meta: { description: "Global new" } },
  { tree: bareTree, name: "fresh", scope: "project", meta: { description: "Fresh" } },
];
for (const input of saveCases) {
  await run(input.tree, async ({ root, home, cwd, rel }) => {
    const result = saveSavedWorkflow({
      cwd,
      homeDir: home,
      name: input.name,
      meta: input.meta,
      script,
      scope: input.scope,
    });
    mutations.push({
      op: "save",
      tree: input.tree,
      input: { name: input.name, scope: input.scope, meta: input.meta },
      result: { path: rel(result.path), scope: result.scope, overwritten: result.overwritten },
      files: await snapshot(root),
    });
  });
}
for (const name of ["global-only", "shared", "nope", "bad name"]) {
  await run(tree, async ({ root, home, cwd, rel }) => {
    const result = moveSavedWorkflow({ cwd, homeDir: home, name });
    mutations.push({
      op: "move",
      tree,
      input: { name },
      result: result.ok
        ? { ok: true, from: rel(result.from), to: rel(result.to) }
        : {
            ok: false,
            reason: result.reason,
            ...(result.path === undefined ? {} : { path: rel(result.path) }),
          },
      files: await snapshot(root),
    });
  });
}

// ---- 参数校验（纯函数）----
const argsInputs = [
  {},
  { provided: { a: 1 } },
  { declaration: { a: { type: "string" }, b: { type: "number" } }, provided: { c: 1 } },
  { declaration: { pr: { type: "string", required: true } }, provided: {} },
  { declaration: { pr: { type: "string", required: true } }, provided: { pr: "1" } },
  { declaration: { n: { type: "number", default: "3" } }, provided: {} },
  { declaration: { n: { type: "number", default: 3 } }, provided: {} },
  { declaration: { n: { type: "number", default: 3 } }, provided: { n: "3" } },
  { declaration: { b: { type: "boolean", required: true } }, provided: { b: "yes" } },
  { declaration: { j: { type: "json", required: true } }, provided: { j: null } },
  { declaration: { j: { type: "json" } }, provided: {} },
  { declaration: { j: { type: "json", default: { a: [1, 2] } } }, provided: { extra: { b: 1 } } },
  { declaration: { z: { type: "string", default: "1" }, a: { type: "string", default: "2" } }, provided: {} },
  {
    declaration: { a: { type: "string", required: true }, b: { type: "number" } },
    provided: { b: "x", c: 1 },
  },
  { declaration: {}, provided: { a: 1 } },
];
const args = argsInputs.map((input) => {
  const result = validateWorkflowArgs(input.declaration, input.provided);
  return {
    declaration: input.declaration ?? null,
    provided: input.provided ?? null,
    ...(result.ok ? { ok: true, args: result.args } : { ok: false, errors: result.errors }),
  };
});

const content = `${JSON.stringify({ tree, bareTree, home: "home", cwd: "repo", reads, mutations, args })}\n`;
if (check) {
  if ((await readFile(target, "utf8").catch(() => "")) !== content)
    throw new Error("Rust saved workflow store corpus differs from TS");
} else await writeFile(target, content);
