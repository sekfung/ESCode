import { describe, expect, it } from "vitest";
import type { SvnBranchGraphResult, SvnFileChange } from "@escode/shared";
import {
  buildSvnFileTreeDecorations,
  canShowSvnBlame,
  canShowSvnFileHistory,
  canShowSvnLock,
  getSelectedSvnPaths,
  getSvnBranchLabel,
  getSvnCommitBlockReason,
  getSvnMergeSourceOptions,
  groupSvnChanges,
  matchesSvnLogFilter,
  parseSvnMergeRange,
  reconcileSvnSelection,
  stripSvnPropertySections,
  summarizeSvnChanges,
  toSvnDiffCardModel,
  validateSvnMergeRange,
} from "../src/svn/svnDisplay.js";
import { layoutSvnBranchGraph } from "../src/svn/svnGraphLayout.js";
import { buildSvnAutoRefreshWatchPaths } from "../src/svn/useSvnWorkspace.js";

function change(path: string, status: SvnFileChange["status"], extra: Partial<SvnFileChange> = {}) {
  return {
    path,
    status,
    propsModified: false,
    isDirectory: false,
    additions: 1,
    deletions: 2,
    isBinary: false,
    ...extra,
  } satisfies SvnFileChange;
}

describe("SVN 面板分组与勾选", () => {
  const changes = [
    change("b.txt", "modified"),
    change("new.txt", "unversioned"),
    change("a.txt", "conflicted", { conflict: { kinds: ["text"] } }),
    change("gone.txt", "missing"),
  ];

  it("冲突置顶，改动与未纳入版本控制分组并按路径排序", () => {
    expect(
      groupSvnChanges(changes).map((group) => [group.id, group.changes.map((c) => c.path)]),
    ).toEqual([
      ["conflicted", ["a.txt"]],
      ["changes", ["b.txt", "gone.txt"]],
      ["unversioned", ["new.txt"]],
    ]);
  });

  it("默认勾选已纳入版本控制的改动，刷新后保留用户勾选", () => {
    const initial = reconcileSvnSelection(new Map(), changes);
    expect(getSelectedSvnPaths(initial).sort()).toEqual(["b.txt", "gone.txt"]);
    const userChanged = new Map(initial).set("new.txt", true).set("b.txt", false);
    const next = reconcileSvnSelection(userChanged, [...changes, change("later.txt", "modified")]);
    expect(getSelectedSvnPaths(next).sort()).toEqual(["gone.txt", "later.txt", "new.txt"]);
    // 冲突文件不可勾选
    expect(next.has("a.txt")).toBe(false);
  });

  it("存在冲突或未勾选时禁止提交", () => {
    expect(getSvnCommitBlockReason(changes, ["b.txt"])).toBe("conflicts");
    expect(getSvnCommitBlockReason([change("b.txt", "modified")], [])).toBe("nothing-selected");
    expect(getSvnCommitBlockReason([change("b.txt", "modified")], ["b.txt"])).toBeNull();
  });

  it("汇总行数与冲突数", () => {
    expect(summarizeSvnChanges(changes)).toEqual({
      added: 4,
      removed: 8,
      fileCount: 4,
      conflictCount: 1,
    });
  });
});

describe("SVN 展示辅助", () => {
  it("分支标签", () => {
    expect(getSvnBranchLabel({ kind: "trunk", name: "trunk", relativePath: "trunk" })).toBe(
      "trunk",
    );
    expect(getSvnBranchLabel({ kind: "tag", name: "v1", relativePath: "tags/v1" })).toBe("tag: v1");
    expect(getSvnBranchLabel(undefined)).toBeNull();
  });

  it("diff 去掉属性段并映射成卡片输入", () => {
    const patch =
      "Index: a\n===\n--- a\n+++ a\n@@ -1 +1 @@\n-x\n+y\nProperty changes on: a\n___\nAdded: p\n## -0,0 +1 ##\n+v\n";
    expect(stripSvnPropertySections(patch)).toBe(
      "Index: a\n===\n--- a\n+++ a\n@@ -1 +1 @@\n-x\n+y",
    );
    expect(
      toSvnDiffCardModel({ path: "a", patch, isBinary: false, truncated: false }),
    ).toMatchObject({
      availability: "patch",
      beforeContent: null,
      afterContent: null,
    });
    expect(
      toSvnDiffCardModel({ path: "b", patch: null, isBinary: true, truncated: false })
        ?.availability,
    ).toBe("binary");
    expect(
      toSvnDiffCardModel({
        path: "c",
        patch: "Property changes on: c\n",
        isBinary: false,
        truncated: false,
      })?.availability,
    ).toBe("unavailable");
  });

  it("历史筛选只匹配作者、信息或修订号", () => {
    const entry = { author: "Alice", message: "Fix login", revision: 42 };
    expect(matchesSvnLogFilter(entry, "alice")).toBe(true);
    expect(matchesSvnLogFilter(entry, "LOGIN")).toBe(true);
    expect(matchesSvnLogFilter(entry, "r42")).toBe(true);
    expect(matchesSvnLogFilter(entry, "bob")).toBe(false);
    expect(matchesSvnLogFilter(entry, "  ")).toBe(true);
  });

  it("文件历史与 Blame 入口按状态与类型收敛", () => {
    expect(canShowSvnFileHistory(change("a.txt", "modified"))).toBe(true);
    expect(canShowSvnFileHistory(change("a.txt", "deleted"))).toBe(true);
    expect(canShowSvnFileHistory(change("a.txt", "missing"))).toBe(true);
    expect(canShowSvnFileHistory(change("new.txt", "added"))).toBe(false);
    expect(canShowSvnFileHistory(change("new.txt", "unversioned"))).toBe(false);
    expect(canShowSvnBlame(change("a.txt", "modified"))).toBe(true);
    expect(canShowSvnBlame(change("dir", "modified", { isDirectory: true }))).toBe(false);
    expect(canShowSvnBlame(change("a.txt", "deleted"))).toBe(false);
    expect(canShowSvnBlame(change("a.txt", "missing"))).toBe(false);
    expect(canShowSvnLock(change("a.txt", "modified"))).toBe(true);
    expect(canShowSvnLock(change("a.txt", "conflicted"))).toBe(false);
    expect(canShowSvnLock(change("a.txt", "deleted"))).toBe(false);
  });

  it("合并源排除当前分支，非标准布局返回空列表", () => {
    const branches = {
      layout: "standard" as const,
      current: { kind: "trunk" as const, name: "trunk", relativePath: "trunk" },
      branches: [
        { kind: "trunk" as const, name: "trunk", relativePath: "trunk" },
        { kind: "branch" as const, name: "feature", relativePath: "branches/feature" },
      ],
    };
    expect(getSvnMergeSourceOptions(branches, branches.current)).toEqual([
      { relativePath: "branches/feature", label: "feature" },
    ]);
    expect(getSvnMergeSourceOptions({ ...branches, layout: "non-standard" }, null)).toEqual([]);
    expect(getSvnMergeSourceOptions(null, null)).toEqual([]);
  });

  it("合并修订范围要求两端正整数且 start ≤ end", () => {
    expect(validateSvnMergeRange("4", "4")).toBeNull();
    expect(validateSvnMergeRange("4", "6")).toBeNull();
    expect(validateSvnMergeRange("", "6")).toBe("missing");
    expect(validateSvnMergeRange("4", "")).toBe("missing");
    expect(validateSvnMergeRange("0", "6")).toBe("invalid");
    expect(validateSvnMergeRange("4.5", "6")).toBe("invalid");
    expect(validateSvnMergeRange("6", "4")).toBe("invalid");
    expect(parseSvnMergeRange("4", "6")).toEqual({ startRevision: 4, endRevision: 6 });
    expect(parseSvnMergeRange("6", "4")).toBeNull();
  });

  it("分支图谱按 trunk/branch/tag 分行，边只连接已知节点", () => {
    const graph: SvnBranchGraphResult = {
      layout: "standard",
      truncated: false,
      nodes: [
        { relativePath: "trunk", kind: "trunk", name: "trunk" },
        {
          relativePath: "branches/feature",
          kind: "branch",
          name: "feature",
          createdFrom: "trunk",
          createdRevision: 3,
        },
        { relativePath: "tags/v1.0", kind: "tag", name: "v1.0" },
      ],
      edges: [
        { kind: "copy", from: "trunk", to: "branches/feature", revision: 3 },
        { kind: "merge", from: "branches/feature", to: "trunk", revisions: "4-5" },
      ],
    };
    const layout = layoutSvnBranchGraph(graph);
    expect(layout.width).toBeGreaterThan(0);
    expect(layout.height).toBeGreaterThan(0);
    expect(layout.nodes).toHaveLength(3);
    expect(layout.edges).toHaveLength(2);
    const trunk = layout.nodes.find((entry) => entry.node.relativePath === "trunk")!;
    const branch = layout.nodes.find((entry) => entry.node.relativePath === "branches/feature")!;
    const tag = layout.nodes.find((entry) => entry.node.relativePath === "tags/v1.0")!;
    expect(trunk.y).toBeLessThan(branch.y);
    expect(branch.y).toBeLessThan(tag.y);
    expect(layout.edges[0]!.path.startsWith("M ")).toBe(true);

    const partial = layoutSvnBranchGraph({
      ...graph,
      edges: [...graph.edges, { kind: "merge", from: "trunk", to: "missing", revisions: "1" }],
    });
    expect(partial.edges).toHaveLength(2);
  });

  it("文件树标记复用改动结果并标出忽略文件", () => {
    const decorations = buildSvnFileTreeDecorations(
      "/wc",
      [
        change("src/a.ts", "modified"),
        change("new.txt", "unversioned"),
        change(".", "props-modified"),
      ],
      ["debug.log"],
    );
    expect(Object.fromEntries(decorations.statusByPath)).toEqual({
      "/wc/src/a.ts": "modified",
      "/wc/new.txt": "untracked",
    });
    expect([...decorations.ignoredPathSet]).toEqual(["/wc/debug.log"]);
  });
});

describe("SVN 自动刷新监听路径", () => {
  it("非 Linux 递归监听 workspace，Linux 只监听根目录与 .svn", () => {
    expect(buildSvnAutoRefreshWatchPaths("/wc", "/wc", null)).toEqual([]);
    expect(buildSvnAutoRefreshWatchPaths("/wc/sub", "/wc", "darwin")).toEqual([
      { path: "/wc/sub", recursive: true },
    ]);
    expect(buildSvnAutoRefreshWatchPaths("/wc/sub", "/wc", "linux")).toEqual([
      { path: "/wc/sub", recursive: false },
      { path: "/wc/.svn", recursive: false },
    ]);
  });
});
