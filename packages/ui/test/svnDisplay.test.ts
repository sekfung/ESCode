import { describe, expect, it } from "vitest";
import type { SvnFileChange } from "@escode/shared";
import {
  buildSvnFileTreeDecorations,
  getSelectedSvnPaths,
  getSvnBranchLabel,
  getSvnCommitBlockReason,
  groupSvnChanges,
  matchesSvnLogFilter,
  reconcileSvnSelection,
  stripSvnPropertySections,
  summarizeSvnChanges,
  toSvnDiffCardModel,
} from "../src/svn/svnDisplay.js";
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
