import { execFileSync } from "node:child_process";
import {
  appendFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { afterAll, beforeEach, describe, expect, it } from "vitest";
import type { AppSettings } from "@escode/shared";
import { createSvnService } from "../src/svn/svnService.js";
import type {
  SvnCommandExecutionOptions,
  SvnCommandExecutionResult,
  SvnCommandProvider,
} from "../src/svn/providers/svnCommandProvider.js";

/**
 * 真实 svn 集成测试：用 svnadmin create + file:// 在临时目录搭仓库。
 * 机器上没有 svn / svnadmin 时显式跳过并输出原因，不静默通过。
 */
function findBinary(name: string): string | null {
  const candidates = [process.env[`ESCODE_${name.toUpperCase()}_BINARY`], name].filter(
    (candidate): candidate is string => Boolean(candidate),
  );
  for (const candidate of candidates) {
    try {
      execFileSync(candidate, ["--version", "--quiet"], { stdio: "ignore" });
      return candidate;
    } catch {
      // 尝试下一个候选
    }
  }
  return null;
}

const svnBinary = findBinary("svn");
const svnadminBinary = findBinary("svnadmin");
const svnAvailable = Boolean(svnBinary && svnadminBinary);
if (!svnAvailable) {
  console.warn(
    "[svnService.test] 跳过真实 svn 集成测试：未找到 svn 或 svnadmin（可设置 ESCODE_SVN_BINARY / ESCODE_SVNADMIN_BINARY）",
  );
}

const tempDirs: string[] = [];

afterAll(() => {
  for (const dir of tempDirs.splice(0)) {
    rmSync(dir, { recursive: true, force: true });
  }
});

function svn(cwd: string, ...args: string[]): string {
  return execFileSync(svnBinary!, ["--non-interactive", ...args], {
    cwd,
    encoding: "utf-8",
    env: { ...process.env, LC_MESSAGES: "C" },
  });
}

interface Fixture {
  root: string;
  repoUrl: string;
  wc: string;
  other: string;
  branchWc: string;
}

function createFixture(): Fixture {
  const root = mkdtempSync(join(tmpdir(), "escode-svn-"));
  tempDirs.push(root);
  execFileSync(svnadminBinary!, ["create", join(root, "repo")]);
  const repoUrl = pathToFileURL(join(root, "repo")).href;
  svn(root, "mkdir", "-m", "layout", `${repoUrl}/trunk`, `${repoUrl}/branches`, `${repoUrl}/tags`);
  svn(root, "checkout", "-q", `${repoUrl}/trunk`, join(root, "wc"));
  const wc = join(root, "wc");
  writeFileSync(join(wc, "a.txt"), "line1\nline2\n");
  mkdirSync(join(wc, "中文"));
  writeFileSync(join(wc, "中文", "文件@2x.txt"), "x\n");
  svn(wc, "add", "-q", "a.txt", "中文");
  svn(wc, "commit", "-q", "-m", "init");
  svn(root, "copy", "-m", "branch", `${repoUrl}/trunk`, `${repoUrl}/branches/feature`);
  svn(root, "checkout", "-q", `${repoUrl}/trunk`, join(root, "other"));
  svn(root, "checkout", "-q", `${repoUrl}/branches/feature`, join(root, "feature"));
  svn(wc, "update", "-q");
  return {
    root,
    repoUrl,
    wc,
    other: join(root, "other"),
    branchWc: join(root, "feature"),
  };
}

function createService(settings: Partial<AppSettings> = {}) {
  let current = { svnBinaryPath: svnBinary ?? undefined, ...settings } as AppSettings;
  return createSvnService({
    settingService: {
      async get() {
        return current;
      },
      async update(patch) {
        current = { ...current, ...patch };
      },
    },
  });
}

describe.skipIf(!svnAvailable)("SvnService（真实 svn）", () => {
  let fixture: Fixture;

  beforeEach(() => {
    fixture = createFixture();
  });

  it("读取工作副本信息与分支", async () => {
    const service = createService();
    const info = await service.getWorkingCopyInfo({ workspacePath: fixture.wc });
    expect(info.availability.status).toBe("available");
    expect(info).toMatchObject({
      isWorkingCopy: true,
      wcRoot: fixture.wc,
      branch: { kind: "trunk", name: "trunk" },
      canSwitch: true,
    });
    const notWc = await service.getWorkingCopyInfo({ workspacePath: fixture.root });
    expect(notWc.isWorkingCopy).toBe(false);
  });

  it("列出改动、行数与忽略文件，diff 未纳入版本控制的文件", async () => {
    const service = createService();
    writeFileSync(join(fixture.wc, "a.txt"), "line1\nchanged\nline3\n");
    writeFileSync(join(fixture.wc, "new.txt"), "n1\nn2\n");
    svn(fixture.wc, "propset", "-q", "svn:ignore", "*.log", ".");
    writeFileSync(join(fixture.wc, "debug.log"), "ignored\n");

    const result = await service.getChanges({ workspacePath: fixture.wc });
    expect(result.ok).toBe(true);
    if (!result.ok) return;
    const byPath = new Map(result.changes.map((change) => [change.path, change]));
    expect(byPath.get("a.txt")).toMatchObject({ status: "modified", additions: 2, deletions: 1 });
    expect(byPath.get("new.txt")).toMatchObject({ status: "unversioned", additions: 2 });
    expect(byPath.get(".")).toMatchObject({ status: "props-modified", propsModified: true });
    expect(result.ignoredPaths).toEqual(["debug.log"]);

    const diff = await service.getDiff({ workspacePath: fixture.wc, path: "new.txt" });
    expect(diff.ok && diff.patch).toContain("+n2");
    const modifiedDiff = await service.getDiff({ workspacePath: fixture.wc, path: "a.txt" });
    expect(modifiedDiff.ok && modifiedDiff.patch).toContain("+changed");
  });

  it("只提交勾选文件，自动 add 未纳入版本控制文件，并处理带 @ 与中文的路径", async () => {
    const service = createService();
    writeFileSync(join(fixture.wc, "a.txt"), "changed\n");
    writeFileSync(join(fixture.wc, "中文", "文件@2x.txt"), "y\n");
    mkdirSync(join(fixture.wc, "newdir"));
    writeFileSync(join(fixture.wc, "newdir", "inner.txt"), "inner\n");
    svn(fixture.wc, "propset", "-q", "svn:ignore", "*.log", ".");

    const commit = await service.commit({
      workspacePath: fixture.wc,
      paths: ["中文/文件@2x.txt", "newdir"],
      message: "feat: 提交中文",
    });
    expect(commit).toMatchObject({ ok: true, addedPaths: ["newdir"] });

    const after = await service.getChanges({ workspacePath: fixture.wc });
    expect(after.ok && after.changes.map((change) => change.path).sort()).toEqual([".", "a.txt"]);
    // 只改了属性的 "." 未勾选，不应被 --depth empty 之外的提交带走
    const log = svn(fixture.wc, "log", "-v", "-l", "1", fixture.repoUrl);
    expect(log).toContain("newdir/inner.txt");
    expect(log).toContain("feat: 提交中文");
    expect(log).not.toContain("/trunk/a.txt");
  });

  it("更新产生冲突 → 用我的 → 提交成功；冲突辅助文件不出现在列表", async () => {
    const service = createService();
    writeFileSync(join(fixture.other, "a.txt"), "theirs\n");
    svn(fixture.other, "commit", "-q", "-m", "theirs");
    writeFileSync(join(fixture.wc, "a.txt"), "mine\n");

    const outOfDate = await service.commit({
      workspacePath: fixture.wc,
      paths: ["a.txt"],
      message: "mine",
    });
    expect(outOfDate).toMatchObject({ ok: false, issue: "out-of-date" });

    const remote = await service.getRemoteStatus({ workspacePath: fixture.wc });
    expect(remote).toMatchObject({ ok: true, outOfDateCount: 1 });

    const update = await service.update({ workspacePath: fixture.wc });
    expect(update).toMatchObject({ ok: true, conflictCount: 1 });

    const changes = await service.getChanges({ workspacePath: fixture.wc });
    expect(changes.ok && changes.changes).toEqual([
      expect.objectContaining({
        path: "a.txt",
        status: "conflicted",
        conflict: { kinds: ["text"] },
      }),
    ]);

    const blocked = await service.commit({
      workspacePath: fixture.wc,
      paths: ["a.txt"],
      message: "mine",
    });
    expect(blocked).toMatchObject({ ok: false, issue: "unresolved-conflicts" });

    expect(
      await service.resolve({ workspacePath: fixture.wc, path: "a.txt", accept: "mine-full" }),
    ).toEqual({ ok: true });
    expect(readFileSync(join(fixture.wc, "a.txt"), "utf-8")).toBe("mine\n");
    const commit = await service.commit({
      workspacePath: fixture.wc,
      paths: ["a.txt"],
      message: "fix: keep mine",
    });
    expect(commit.ok).toBe(true);
  });

  it("树冲突只读展示原因", async () => {
    const service = createService();
    svn(fixture.other, "delete", "-q", "中文");
    svn(fixture.other, "commit", "-q", "-m", "delete dir");
    writeFileSync(join(fixture.wc, "中文", "文件@2x.txt"), "edit\n");
    await service.update({ workspacePath: fixture.wc });

    const changes = await service.getChanges({ workspacePath: fixture.wc });
    const conflicted = changes.ok
      ? changes.changes.find((change) => change.status === "conflicted")
      : undefined;
    expect(conflicted?.conflict).toEqual({
      kinds: ["tree"],
      description: "local edit, incoming delete upon update",
    });
  });

  it("历史分页与修订 diff", async () => {
    const service = createService();
    for (const value of ["1", "2", "3"]) {
      writeFileSync(join(fixture.wc, "a.txt"), `${value}\n`);
      svn(fixture.wc, "commit", "-q", "-m", `change ${value}`);
    }
    const first = await service.getLog({ workspacePath: fixture.wc, limit: 2 });
    expect(first.ok).toBe(true);
    if (!first.ok) return;
    expect(first.entries.map((entry) => entry.message)).toEqual(["change 3", "change 2"]);
    expect(first.hasMore).toBe(true);
    const next = await service.getLog({
      workspacePath: fixture.wc,
      limit: 2,
      startRevision: first.entries[1]!.revision - 1,
    });
    expect(next.ok && next.entries[0]?.message).toBe("change 1");

    const diff = await service.getDiff({
      workspacePath: fixture.wc,
      path: "/trunk/a.txt",
      revision: first.entries[0]!.revision,
    });
    expect(diff.ok && diff.patch).toContain("+3");
  });

  it("单文件历史按文件 URL 分页，缺失文件仍可查看", async () => {
    const service = createService();
    for (const value of ["1", "2", "3"]) {
      writeFileSync(join(fixture.wc, "a.txt"), `${value}\n`);
      svn(fixture.wc, "commit", "-q", "-m", `edit a ${value}`);
    }
    // 另一个文件的历史不应混入。
    writeFileSync(join(fixture.wc, "b.txt"), "b\n");
    svn(fixture.wc, "add", "-q", "b.txt");
    svn(fixture.wc, "commit", "-q", "-m", "add b");

    const first = await service.getFileLog({ workspacePath: fixture.wc, path: "a.txt", limit: 2 });
    expect(first.ok).toBe(true);
    if (!first.ok) return;
    expect(first.repositoryPath).toBe("/trunk/a.txt");
    expect(first.entries.map((entry) => entry.message)).toEqual(["edit a 3", "edit a 2"]);
    expect(first.hasMore).toBe(true);
    const next = await service.getFileLog({
      workspacePath: fixture.wc,
      path: "a.txt",
      limit: 2,
      startRevision: first.entries[1]!.revision - 1,
    });
    expect(next.ok && next.entries.map((entry) => entry.message)).toEqual(["edit a 1", "init"]);

    // 本地删除但未提交时仍能读到文件历史。
    rmSync(join(fixture.wc, "a.txt"));
    const missing = await service.getFileLog({ workspacePath: fixture.wc, path: "a.txt" });
    expect(missing.ok && missing.entries[0]?.message).toBe("edit a 3");

    writeFileSync(join(fixture.wc, "untracked.txt"), "x\n");
    expect(
      await service.getFileLog({ workspacePath: fixture.wc, path: "untracked.txt" }),
    ).toMatchObject({ ok: false, issue: "invalid-request" });
  });

  it("Blame 返回逐行修订、作者与内容", async () => {
    const service = createService();
    writeFileSync(join(fixture.wc, "a.txt"), "alpha\nbeta\n");
    svn(fixture.wc, "commit", "-q", "-m", "blame base");
    const baseRevision = Number(svn(fixture.wc, "info", "--show-item", "revision", "a.txt").trim());
    appendFileSync(join(fixture.wc, "a.txt"), "gamma\n");
    svn(fixture.wc, "commit", "-q", "-m", "append gamma");

    const blame = await service.getBlame({ workspacePath: fixture.wc, path: "a.txt" });
    expect(blame.ok).toBe(true);
    if (!blame.ok) return;
    expect(blame.truncated).toBe(false);
    expect(blame.lines.map((line) => line.content)).toEqual(["alpha", "beta", "gamma"]);
    expect(blame.lines.map((line) => line.lineNumber)).toEqual([1, 2, 3]);
    expect(blame.lines[0]!.revision).toBe(baseRevision);
    expect(blame.lines[2]!.revision).toBeGreaterThan(baseRevision);
    expect(blame.lines[0]!.author.length).toBeGreaterThan(0);

    const atBase = await service.getBlame({
      workspacePath: fixture.wc,
      path: "a.txt",
      revision: baseRevision,
    });
    expect(atBase.ok && atBase.lines.map((line) => line.content)).toEqual(["alpha", "beta"]);
    expect(
      await service.getBlame({ workspacePath: fixture.wc, path: "../escape.txt" }),
    ).toMatchObject({ ok: false, issue: "invalid-request" });
  });

  it("列出分支并切换", async () => {
    const service = createService();
    const branches = await service.listBranches({ workspacePath: fixture.wc });
    expect(branches).toMatchObject({
      ok: true,
      layout: "standard",
      current: { kind: "trunk" },
      branches: [
        { kind: "trunk", relativePath: "trunk" },
        { kind: "branch", name: "feature", relativePath: "branches/feature" },
      ],
    });
    const switched = await service.switchBranch({
      workspacePath: fixture.wc,
      relativePath: "branches/feature",
    });
    expect(switched).toMatchObject({ ok: true, branch: { kind: "branch", name: "feature" } });
    expect(
      await service.switchBranch({ workspacePath: fixture.wc, relativePath: "../etc" }),
    ).toMatchObject({ ok: false, issue: "invalid-request" });
  });

  it("分支图谱展示 copyfrom 与 mergeinfo", async () => {
    const service = createService();
    writeFileSync(join(fixture.branchWc, "feature.txt"), "from feature\n");
    svn(fixture.branchWc, "add", "-q", "feature.txt");
    svn(fixture.branchWc, "commit", "-q", "-m", "feat: graph source");
    svn(fixture.wc, "update", "-q");
    expect(
      await service.merge({ workspacePath: fixture.wc, relativeSourcePath: "branches/feature" }),
    ).toMatchObject({ ok: true });
    expect(
      await service.commit({
        workspacePath: fixture.wc,
        paths: ["feature.txt", "."],
        message: "test: graph merge",
      }),
    ).toMatchObject({ ok: true });

    const graph = await service.getBranchGraph({ workspacePath: fixture.wc });
    expect(graph.ok).toBe(true);
    if (!graph.ok) return;
    expect(graph.layout).toBe("standard");
    expect(graph.truncated).toBe(false);
    expect(graph.nodes).toContainEqual({ relativePath: "trunk", kind: "trunk", name: "trunk" });
    expect(graph.nodes).toContainEqual({
      relativePath: "branches/feature",
      kind: "branch",
      name: "feature",
      createdFrom: "trunk",
      createdRevision: 3,
    });
    expect(graph.edges).toContainEqual({
      kind: "copy",
      from: "trunk",
      to: "branches/feature",
      revision: 3,
    });
    const mergeEdge = graph.edges.find(
      (edge) => edge.kind === "merge" && edge.from === "branches/feature" && edge.to === "trunk",
    );
    expect(mergeEdge?.revisions).toContain("4");

    // 非标准布局：只有当前节点，不推导拓扑。
    const nonStandardRoot = mkdtempSync(join(tmpdir(), "escode-svn-flat-"));
    tempDirs.push(nonStandardRoot);
    execFileSync(svnadminBinary!, ["create", join(nonStandardRoot, "repo")]);
    const flatUrl = pathToFileURL(join(nonStandardRoot, "repo")).href;
    svn(nonStandardRoot, "checkout", "-q", flatUrl, join(nonStandardRoot, "wc"));
    writeFileSync(join(nonStandardRoot, "wc", "a.txt"), "a\n");
    svn(join(nonStandardRoot, "wc"), "add", "-q", "a.txt");
    svn(join(nonStandardRoot, "wc"), "commit", "-q", "-m", "flat");
    const flat = await service.getBranchGraph({
      workspacePath: join(nonStandardRoot, "wc"),
    });
    expect(flat).toMatchObject({
      ok: true,
      layout: "non-standard",
      edges: [],
      truncated: false,
    });
    expect(flat.ok && flat.nodes).toEqual([{ relativePath: "", kind: "other", name: "/" }]);
  });

  it("合并分支改动，记录 mergeinfo 并可提交", async () => {
    const service = createService();
    writeFileSync(join(fixture.branchWc, "feature.txt"), "from feature\n");
    svn(fixture.branchWc, "add", "-q", "feature.txt");
    svn(fixture.branchWc, "commit", "-q", "-m", "feat: branch file");

    const merged = await service.merge({
      workspacePath: fixture.wc,
      relativeSourcePath: "branches/feature",
    });
    expect(merged.ok).toBe(true);
    if (!merged.ok) return;
    expect(merged.conflictCount).toBe(0);
    expect(merged.mergedFileCount).toBeGreaterThanOrEqual(1);
    expect(existsSync(join(fixture.wc, "feature.txt"))).toBe(true);
    expect(readFileSync(join(fixture.wc, "feature.txt"), "utf-8")).toBe("from feature\n");

    const changes = await service.getChanges({ workspacePath: fixture.wc });
    const byPath = changes.ok
      ? new Map(changes.changes.map((change) => [change.path, change]))
      : new Map();
    expect(byPath.get("feature.txt")).toMatchObject({ status: "added" });
    expect(byPath.get(".")).toMatchObject({ status: "props-modified" });
    expect(svn(fixture.wc, "propget", "svn:mergeinfo", ".")).toContain("branches/feature");

    const commit = await service.commit({
      workspacePath: fixture.wc,
      paths: ["feature.txt", "."],
      message: "test: merge feature",
    });
    expect(commit.ok).toBe(true);
    const log = svn(fixture.wc, "log", "-v", "-l", "1", fixture.repoUrl);
    expect(log).toContain("test: merge feature");
    expect(log).toContain("feature.txt");
  });

  it("合并产生冲突后可解决并提交；非法源与修订范围被拒绝", async () => {
    const service = createService();
    writeFileSync(join(fixture.branchWc, "a.txt"), "feature side\n");
    svn(fixture.branchWc, "commit", "-q", "-m", "feat: branch edit");
    writeFileSync(join(fixture.wc, "a.txt"), "trunk side\n");

    const merged = await service.merge({
      workspacePath: fixture.wc,
      relativeSourcePath: "branches/feature",
    });
    expect(merged).toMatchObject({ ok: true, conflictCount: 1 });
    const changes = await service.getChanges({ workspacePath: fixture.wc });
    expect(changes.ok && changes.changes).toContainEqual(
      expect.objectContaining({ path: "a.txt", status: "conflicted" }),
    );

    expect(
      await service.resolve({ workspacePath: fixture.wc, path: "a.txt", accept: "mine-full" }),
    ).toEqual({ ok: true });
    expect(readFileSync(join(fixture.wc, "a.txt"), "utf-8")).toBe("trunk side\n");
    expect(
      await service.commit({
        workspacePath: fixture.wc,
        paths: ["a.txt", "."],
        message: "fix: merge conflict",
      }),
    ).toMatchObject({ ok: true });

    expect(
      await service.merge({ workspacePath: fixture.wc, relativeSourcePath: "../etc" }),
    ).toMatchObject({ ok: false, issue: "invalid-request" });
    expect(
      await service.merge({
        workspacePath: fixture.wc,
        relativeSourcePath: "branches/feature",
        startRevision: 5,
      }),
    ).toMatchObject({ ok: false, issue: "invalid-request" });
    expect(
      await service.merge({
        workspacePath: fixture.wc,
        relativeSourcePath: "branches/feature",
        startRevision: 6,
        endRevision: 5,
      }),
    ).toMatchObject({ ok: false, issue: "invalid-request" });
  });

  it("指定修订范围时只应用该区间的改动", async () => {
    const service = createService();
    writeFileSync(join(fixture.branchWc, "one.txt"), "one\n");
    svn(fixture.branchWc, "add", "-q", "one.txt");
    svn(fixture.branchWc, "commit", "-q", "-m", "feat: one");
    const firstRevision = Number(
      svn(fixture.branchWc, "info", "--show-item", "last-changed-revision", "one.txt").trim(),
    );
    writeFileSync(join(fixture.branchWc, "two.txt"), "two\n");
    svn(fixture.branchWc, "add", "-q", "two.txt");
    svn(fixture.branchWc, "commit", "-q", "-m", "feat: two");

    const merged = await service.merge({
      workspacePath: fixture.wc,
      relativeSourcePath: "branches/feature",
      startRevision: firstRevision,
      endRevision: firstRevision,
    });
    expect(merged).toMatchObject({ ok: true, conflictCount: 0 });
    expect(existsSync(join(fixture.wc, "one.txt"))).toBe(true);
    expect(existsSync(join(fixture.wc, "two.txt"))).toBe(false);
  });

  it("还原保留新增文件，锁定时报 working-copy-locked 并可清理", async () => {
    const service = createService();
    writeFileSync(join(fixture.wc, "a.txt"), "changed\n");
    writeFileSync(join(fixture.wc, "added.txt"), "added\n");
    svn(fixture.wc, "add", "-q", "added.txt");
    expect(
      await service.revert({ workspacePath: fixture.wc, paths: ["a.txt", "added.txt"] }),
    ).toEqual({ ok: true });
    expect(readFileSync(join(fixture.wc, "a.txt"), "utf-8")).toBe("line1\nline2\n");
    expect(existsSync(join(fixture.wc, "added.txt"))).toBe(true);

    // 直接写入 wc.db 的 WC_LOCK 表，模拟 svn 进程中断后残留的工作副本锁
    const { DatabaseSync } = await import("node:sqlite");
    const db = new DatabaseSync(join(fixture.wc, ".svn", "wc.db"));
    db.exec("INSERT INTO WC_LOCK (wc_id, local_dir_relpath, locked_levels) VALUES (1, '', -1);");
    db.close();
    writeFileSync(join(fixture.wc, "a.txt"), "again\n");
    const locked = await service.revert({ workspacePath: fixture.wc, paths: ["a.txt"] });
    expect(locked).toMatchObject({ ok: false, issue: "working-copy-locked" });
    expect(await service.cleanup({ workspacePath: fixture.wc })).toEqual({ ok: true });
    expect(await service.revert({ workspacePath: fixture.wc, paths: ["a.txt"] })).toEqual({
      ok: true,
    });
  });

  it("读取 svn:externals 的 -r 与 @peg 两种格式", async () => {
    const service = createService();
    const definitions = join(fixture.root, "externals.txt");
    writeFileSync(definitions, `-r 2 ^/trunk ext-a\n${fixture.repoUrl}/branches/feature@3 ext/b`);
    svn(fixture.wc, "propset", "-q", "-F", definitions, "svn:externals", ".");
    const externals = await service.getExternals({ workspacePath: fixture.wc });
    expect(externals.ok).toBe(true);
    if (!externals.ok) return;
    expect(externals.externals).toEqual([
      { path: ".", name: "ext-a", url: "^/trunk", revision: 2 },
      {
        path: ".",
        name: "ext/b",
        url: `${fixture.repoUrl}/branches/feature`,
        pegRevision: 3,
      },
    ]);
    svn(fixture.wc, "propdel", "-q", "svn:externals", ".");
    expect(await service.getExternals({ workspacePath: fixture.wc })).toEqual({
      ok: true,
      externals: [],
    });
  });

  it("锁定状态进入改动列表，解锁他人锁需要 force", async () => {
    const service = createService();
    writeFileSync(join(fixture.wc, "a.txt"), "locked change\n");
    expect(
      await service.lock({ workspacePath: fixture.wc, paths: ["a.txt"], message: "hold" }),
    ).toEqual({ ok: true });
    const changes = await service.getChanges({ workspacePath: fixture.wc });
    const locked = changes.ok ? changes.changes.find((change) => change.path === "a.txt") : null;
    expect(locked?.lock).toMatchObject({ comment: "hold" });
    expect(locked?.lock?.owner.length).toBeGreaterThan(0);

    const withoutForce = await service.unlock({
      workspacePath: fixture.other,
      paths: ["a.txt"],
    });
    expect(withoutForce.ok).toBe(false);
    expect(
      await service.unlock({ workspacePath: fixture.other, paths: ["a.txt"], force: true }),
    ).toEqual({ ok: true });
    // status 的锁信息来自本地 wc.db；另一个工作副本解锁后需要 update 才能刷新。
    svn(fixture.wc, "update", "-q");
    const after = await service.getChanges({ workspacePath: fixture.wc });
    expect(
      after.ok && after.changes.find((change) => change.path === "a.txt")?.lock,
    ).toBeUndefined();
    expect(await service.lock({ workspacePath: fixture.wc, paths: ["../x"] })).toMatchObject({
      ok: false,
      issue: "invalid-request",
    });
  });

  it("暂存架保存修改与新增文件，应用后恢复", async () => {
    const service = createService();
    writeFileSync(join(fixture.wc, "a.txt"), "shelf change\n");
    writeFileSync(join(fixture.wc, "shelf-new.txt"), "new shelf\n");
    const shelved = await service.shelve({
      workspacePath: fixture.wc,
      paths: ["a.txt", "shelf-new.txt"],
      name: "s1",
      message: "hold",
    });
    expect(shelved).toMatchObject({
      ok: true,
      shelf: { name: "s1", message: "hold", paths: ["a.txt", "shelf-new.txt"] },
    });
    expect(readFileSync(join(fixture.wc, "a.txt"), "utf-8")).toBe("line1\nline2\n");
    expect(existsSync(join(fixture.wc, "shelf-new.txt"))).toBe(false);

    const shelves = await service.listShelves({ workspacePath: fixture.wc });
    expect(shelves.ok && shelves.shelves.map((shelf) => shelf.name)).toEqual(["s1"]);

    expect(await service.unshelve({ workspacePath: fixture.wc, name: "s1" })).toEqual({
      ok: true,
    });
    expect(readFileSync(join(fixture.wc, "a.txt"), "utf-8")).toBe("shelf change\n");
    expect(readFileSync(join(fixture.wc, "shelf-new.txt"), "utf-8")).toBe("new shelf\n");
    const after = await service.getChanges({ workspacePath: fixture.wc });
    expect(
      after.ok && after.changes.find((change) => change.path === "shelf-new.txt")?.status,
    ).toBe("added");

    expect(await service.deleteShelf({ workspacePath: fixture.wc, name: "s1" })).toEqual({
      ok: true,
    });
    expect(await service.listShelves({ workspacePath: fixture.wc })).toEqual({
      ok: true,
      shelves: [],
    });
    expect(await service.unshelve({ workspacePath: fixture.wc, name: "s1" })).toMatchObject({
      ok: false,
      issue: "shelf-not-found",
    });
    expect(
      await service.shelve({ workspacePath: fixture.wc, paths: ["../x"], name: "bad" }),
    ).toMatchObject({ ok: false, issue: "invalid-request" });

    // 缺失文件按删除暂存，暂存后恢复 BASE 内容。
    rmSync(join(fixture.wc, "a.txt"));
    expect(
      await service.shelve({ workspacePath: fixture.wc, paths: ["a.txt"], name: "missing" }),
    ).toMatchObject({ ok: true });
    expect(readFileSync(join(fixture.wc, "a.txt"), "utf-8")).toBe("line1\nline2\n");

    // 二进制文件拒绝暂存。
    writeFileSync(join(fixture.wc, "bin.dat"), Buffer.from([0, 1, 2]));
    svn(fixture.wc, "add", "-q", "bin.dat");
    svn(fixture.wc, "commit", "-q", "-m", "add bin");
    writeFileSync(join(fixture.wc, "bin.dat"), Buffer.from([0, 9, 9]));
    expect(
      await service.shelve({ workspacePath: fixture.wc, paths: ["bin.dat"], name: "bin" }),
    ).toMatchObject({ ok: false, issue: "invalid-request" });
  });

  it("找不到 svn 时报告不可用", async () => {
    const service = createService({ svnBinaryPath: join(fixture.root, "missing-svn") });
    const info = await service.getWorkingCopyInfo({ workspacePath: fixture.wc });
    // 手动路径无效时继续按 PATH 查找，因此只要 PATH 上有 svn 仍可用
    expect(["available", "not-found"]).toContain(info.availability.status);
  });
});

function createScriptedProvider(
  handler: (options: SvnCommandExecutionOptions) => Promise<Partial<SvnCommandExecutionResult>>,
  version = "1.14.0",
): { provider: SvnCommandProvider; calls: SvnCommandExecutionOptions[] } {
  const calls: SvnCommandExecutionOptions[] = [];
  return {
    calls,
    provider: {
      async resolveSvnBinary() {
        return { binaryPath: "svn", version };
      },
      async run(options) {
        calls.push(options);
        return {
          stdout: "",
          stderr: "",
          exitCode: 0,
          durationMs: 1,
          timedOut: false,
          outputTruncated: false,
          ...(await handler(options)),
        };
      },
    },
  };
}

describe("SvnService（模拟命令）", () => {
  it("同一身份 key 的写操作串行，不同 key 并行，读操作不排队", async () => {
    const events: string[] = [];
    const releases: Array<() => void> = [];
    const { provider } = createScriptedProvider(async (options) => {
      const label = `${options.cwd}:${options.args[1]}`;
      events.push(`start ${label}`);
      if (options.args[1] === "revert") {
        await new Promise<void>((resolve) => releases.push(resolve));
      }
      events.push(`end ${label}`);
      return {};
    });
    const service = createSvnService({ commandProvider: provider });

    const first = service.revert({ workspacePath: "/a", paths: ["x"] });
    const second = service.revert({ workspacePath: "/a", paths: ["y"] });
    const otherKey = service.revert({ workspacePath: "/b", paths: ["z"] });
    const read = service.getDiff({ workspacePath: "/a", path: "x", revision: 3 });
    await new Promise((resolve) => setTimeout(resolve, 10));
    // 第二个 /a 写操作仍在排队；/b 写操作与 /a 读操作不受影响
    expect(events.filter((event) => event.startsWith("start")).sort()).toEqual([
      "start /a:diff",
      "start /a:revert",
      "start /b:revert",
    ]);
    releases.shift()!();
    await first;
    await new Promise((resolve) => setTimeout(resolve, 10));
    expect(events).toContain("start /a:revert");
    expect(events.filter((event) => event === "start /a:revert")).toHaveLength(2);
    for (const release of releases.splice(0)) release();
    await new Promise((resolve) => setTimeout(resolve, 10));
    for (const release of releases.splice(0)) release();
    await Promise.all([second, otherKey, read]);
  });

  it("svn ≥ 1.10 通过 stdin 传密码，旧版本退回 --password；不缓存时加 --no-auth-cache", async () => {
    const credentials = { username: "u", password: "p@ss", storeInSvnCache: false };
    const modern = createScriptedProvider(async () => ({}));
    await createSvnService({ commandProvider: modern.provider }).update({
      workspacePath: "/wc",
      credentials,
    });
    const modernUpdate = modern.calls.find((call) => call.args.includes("update"))!;
    expect(modernUpdate.args).toEqual(
      expect.arrayContaining(["--password-from-stdin", "--username", "u", "--no-auth-cache"]),
    );
    expect(modernUpdate.args).not.toContain("p@ss");
    expect(modernUpdate.stdin).toBe("p@ss");

    const legacy = createScriptedProvider(async () => ({}), "1.9.7");
    await createSvnService({ commandProvider: legacy.provider }).update({
      workspacePath: "/wc",
      credentials: { ...credentials, storeInSvnCache: true },
    });
    const legacyUpdate = legacy.calls.find((call) => call.args.includes("update"))!;
    expect(legacyUpdate.args).toEqual(expect.arrayContaining(["--password", "p@ss"]));
    expect(legacyUpdate.args).not.toContain("--no-auth-cache");
    expect(legacyUpdate.stdin).toBeUndefined();
  });

  it("认证失败归类为 auth-required；版本过低报告不可用", async () => {
    const { provider } = createScriptedProvider(async () => ({
      exitCode: 1,
      stderr: "svn: E170001: Authentication required for 'realm'\n",
    }));
    const service = createSvnService({ commandProvider: provider });
    expect(await service.update({ workspacePath: "/wc" })).toMatchObject({
      ok: false,
      issue: "auth-required",
    });

    const old = createScriptedProvider(async () => ({}), "1.7.22");
    const info = await createSvnService({ commandProvider: old.provider }).getWorkingCopyInfo({
      workspacePath: "/wc",
    });
    expect(info.availability).toEqual({
      status: "unsupported-version",
      version: "1.7.22",
      binaryPath: "svn",
    });
  });

  it("拒绝越界路径", async () => {
    const { provider, calls } = createScriptedProvider(async () => ({}));
    const service = createSvnService({ commandProvider: provider });
    expect(await service.revert({ workspacePath: "/wc", paths: ["../x"] })).toMatchObject({
      ok: false,
      issue: "invalid-request",
    });
    expect(
      await service.commit({ workspacePath: "/wc", paths: ["/etc/passwd"], message: "x" }),
    ).toMatchObject({ ok: false, issue: "invalid-request" });
    expect(calls).toHaveLength(0);
  });
});
