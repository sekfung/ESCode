import assert from "node:assert/strict";
import test from "node:test";
import { join, resolve } from "node:path";
import { mkdir, mkdtemp, readFile, readdir, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { fixture } from "./zcode-cli-rust-fixture.js";
import { configureRegistry } from "./zcode-cli-rust-registry-fixture.js";

// docs/specs/rust-dynamic-workflow.md 第 2 期：GUI 中枢 `workflows/*`（list / get / updateMeta /
// delete / move）在 Node 与 Rust 上同名同形。`workflows/runs` 是第 4 期的 journal，不在用例里。
// fixture 把 HOME/USERPROFILE 指到临时 root，所以"全局档"落在临时目录，两侧都安全。
process.env.ZCODE_TEST_WAIT_MS ??= "30000";
const nodeBundle = resolve("apps/zcode-cli/packages/cli/dist/zcode.cjs");
const workflowFile = (description, whenToUse) =>
  [
    "/* zcode-workflow",
    `description: ${description}`,
    ...(whenToUse === undefined ? [] : [`whenToUse: ${whenToUse}`]),
    "*/",
    "export default async () => {",
    "  return 1;",
    "}",
    "",
  ].join("\n");

async function observe(kind: "node" | "rust") {
  const root = await mkdtemp(join(tmpdir(), `zcode-saved-workflow-hub-${kind}-`));
  const f =
    kind === "node"
      ? await fixture({
          root,
          command: process.execPath,
          args: ({ cwd }) => [nodeBundle, "app-server", "--stdio", "--cwd", cwd],
          registry: true,
          mode: "yolo",
        })
      : await fixture({ root, registry: true, mode: "yolo" });
  try {
    await configureRegistry(f, false);
    const h = f.start();
    const workspace = { workspacePath: h.workspace, workspaceKey: h.workspace };
    const projectDir = join(h.workspace, ".zcode", "workflows");
    const globalDir = join(root, ".zcode", "workflows");
    await mkdir(projectDir, { recursive: true });
    await writeFile(join(projectDir, "review.dwf.ts"), workflowFile("Review the diff"));

    const normalize = (value: any) => {
      const text = JSON.stringify(value)
        .replaceAll(JSON.stringify(h.workspace).slice(1, -1), "<workspace>")
        .replaceAll(JSON.stringify(root).slice(1, -1), "<root>");
      return JSON.parse(text);
    };
    const ask = async (method: string, params: Record<string, unknown>) =>
      normalize(
        await h.client.request(method, params, { parse: (value: unknown) => value } as any),
      );
    const outcome = async (method: string, params: Record<string, unknown>) => {
      try {
        return { ok: true, value: await ask(method, params) };
      } catch {
        return { ok: false };
      }
    };
    const at = (name: string) => ({ workspace, name });
    const observed: Record<string, unknown> = {};
    observed.listProject = await ask("workflows/list", { workspace });
    observed.listGlobal = await ask("workflows/list", { workspace, scope: "global" });
    observed.get = await ask("workflows/get", at("review"));
    observed.getMissing = await ask("workflows/get", at("nope"));
    observed.getBadName = await ask("workflows/get", at("bad/name"));
    observed.updateMeta = await ask("workflows/updateMeta", {
      ...at("review"),
      meta: { description: "Updated review", whenToUse: "When a PR is open" },
    });
    observed.getAfterUpdate = await ask("workflows/get", at("review"));
    // updateMeta 落盘的正文（读-改-写后的整文件字节）。
    observed.reviewBytes = await readFile(join(projectDir, "review.dwf.ts"), "utf8");
    // 元数据不合 schema：两侧都必须拒绝（错误文案是各自 schema 库的措辞，只比"都失败"）。
    observed.updateMetaInvalid = await outcome("workflows/updateMeta", {
      ...at("review"),
      meta: { description: "" },
    });
    // 全局档：写入后只出现在 global 组，项目组不受影响。
    await mkdir(globalDir, { recursive: true });
    await writeFile(join(globalDir, "global-only.dwf.ts"), workflowFile("Global only"));
    observed.listProjectWithGlobal = await ask("workflows/list", { workspace });
    observed.listGlobalWithGlobal = await ask("workflows/list", { workspace, scope: "global" });
    observed.move = await ask("workflows/move", at("global-only"));
    observed.getMoved = await ask("workflows/get", at("global-only"));
    observed.moveAgain = await ask("workflows/move", at("global-only"));
    // 目标已存在（项目档已有同名）时拒绝搬运，并指认目标路径。
    await writeFile(join(globalDir, "review.dwf.ts"), workflowFile("Global review"));
    observed.moveTargetExists = await ask("workflows/move", at("review"));
    observed.moveBadName = await ask("workflows/move", at("bad/name"));
    observed.deleteReview = await ask("workflows/delete", at("review"));
    observed.deleteAgain = await ask("workflows/delete", at("review"));
    observed.deleteBadName = await ask("workflows/delete", at("bad/name"));
    await h.close();
    // 落盘效果：项目档目录与全局档目录的文件名（删除/覆盖/move 的最终状态）。
    const names = async (dir: string) =>
      (await readdir(dir).catch(() => [] as string[])).sort();
    return {
      observed,
      projectFiles: await names(projectDir),
      globalFiles: await names(globalDir),
      // move 过来的全局档最终落在项目档里（review 已在用例中被删除）。
      movedFile: await readFile(join(projectDir, "global-only.dwf.ts"), "utf8"),
    };
  } finally {
    await f.close();
  }
}

test("workflows/* hub matches Node", async () => {
  const node = await observe("node");
  const rust = await observe("rust");
  // 自检：Node 侧的形状与语义确实是我们期望的那套。
  assert.equal((node.observed.listProject as any).workflows.length, 1);
  assert.equal((node.observed.listProject as any).invalid.length, 0);
  assert.equal((node.observed.get as any).ok, true);
  assert.equal((node.observed.getMissing as any).reason, "not_found");
  assert.equal((node.observed.getBadName as any).reason, "invalid_name");
  assert.equal((node.observed.updateMeta as any).ok, true);
  assert.equal((node.observed.updateMetaInvalid as any).ok, false);
  assert.equal((node.observed.listProjectWithGlobal as any).workflows.length, 1);
  assert.equal((node.observed.listGlobalWithGlobal as any).workflows.length, 1);
  assert.equal((node.observed.move as any).ok, true);
  assert.equal((node.observed.moveAgain as any).reason, "not_found");
  assert.equal((node.observed.moveTargetExists as any).reason, "target_exists");
  assert.equal((node.observed.moveBadName as any).reason, "invalid_name");
  assert.equal((node.observed.deleteReview as any).ok, true);
  assert.equal((node.observed.deleteAgain as any).reason, "not_found");
  assert.equal((node.observed.deleteBadName as any).reason, "invalid_name");
  // 两侧逐字一致（路径按 workspace/root 归一）。
  assert.deepEqual(rust.observed, node.observed);
  assert.deepEqual(rust.projectFiles, node.projectFiles);
  assert.deepEqual(rust.globalFiles, node.globalFiles);
  assert.equal(rust.movedFile, node.movedFile);
});
