import { describe, expect, it } from "vitest";
import { deriveSvnBranchRef, getSvnRelativeRepositoryPath } from "@escode/shared";
import { compareSvnVersions, getSvnCommandEnv } from "../src/svn/config.js";
import { classifySvnError, summarizeSvnStderr } from "../src/svn/repo/svnErrors.js";
import {
  countSvnDiffLines,
  countSvnUpdatedPaths,
  parseSvnCommittedRevision,
  parseSvnInfoXml,
  parseSvnListXml,
  parseSvnLogXml,
  parseSvnStatusXml,
  toSvnItemStatus,
} from "../src/svn/repo/svnParse.js";

describe("svn XML 解析", () => {
  it("解析 info，含非 ASCII 路径与树冲突", () => {
    const entries = parseSvnInfoXml(`<?xml version="1.0" encoding="UTF-8"?>
<info>
<entry kind="dir" path="中文" revision="4">
<url>file:///repo/trunk/%E4%B8%AD%E6%96%87</url>
<repository><root>file:///repo</root><uuid>u-1</uuid></repository>
<wc-info><wcroot-abspath>/wc</wcroot-abspath></wc-info>
<commit revision="2"><author>a</author></commit>
<tree-conflict kind="dir" reason="edit" victim="中文" action="delete" operation="update">
<version revision="2" side="source-left" kind="dir" path-in-repos="trunk/中文" repos-url="file:///repo"/>
</tree-conflict>
</entry>
<entry kind="file" path="a.txt" revision="4">
<conflict operation="update" type="text">
<prev-base-file>/wc/a.txt.r2</prev-base-file>
<prev-wc-file>/wc/a.txt.mine</prev-wc-file>
<cur-base-file>/wc/a.txt.r4</cur-base-file>
</conflict>
</entry>
</info>`);
    expect(entries[0]).toMatchObject({
      path: "中文",
      kind: "dir",
      revision: 4,
      repositoryRoot: "file:///repo",
      repositoryUuid: "u-1",
      wcRoot: "/wc",
      lastChangedRevision: 2,
      conflict: {
        kinds: ["tree"],
        treeConflict: { reason: "edit", action: "delete", operation: "update" },
      },
    });
    expect(entries[1]?.conflict).toEqual({
      kinds: ["text"],
      artifactPaths: ["/wc/a.txt.r2", "/wc/a.txt.mine", "/wc/a.txt.r4"],
    });
  });

  it("解析 status 并映射界面状态", () => {
    const entries = parseSvnStatusXml(`<status><target path=".">
<entry path="."><wc-status item="normal" props="modified"></wc-status></entry>
<entry path="a.txt"><wc-status item="modified" props="none"></wc-status></entry>
<entry path="b&amp;c.txt"><wc-status item="unversioned" props="none"></wc-status></entry>
<entry path="x.log"><wc-status item="ignored" props="none"></wc-status></entry>
<entry path="d"><wc-status item="added" props="none" tree-conflicted="true"></wc-status></entry>
<entry path="ext"><wc-status item="external" props="none"></wc-status></entry>
</target>
<target path="ext"><entry path="ext/inner.txt"><wc-status item="modified" props="none"></wc-status></entry></target>
</status>`);
    expect(entries.map((entry) => [entry.path, toSvnItemStatus(entry)])).toEqual([
      [".", "props-modified"],
      ["a.txt", "modified"],
      ["b&c.txt", "unversioned"],
      ["x.log", null],
      ["d", "conflicted"],
      ["ext", "external"],
      ["ext/inner.txt", "modified"],
    ]);
    expect(entries.find((entry) => entry.path === "ext/inner.txt")?.inExternal).toBe(true);
  });

  it("解析 log 与 list", () => {
    const log = parseSvnLogXml(`<log>
<logentry revision="3"><author>bob</author><date>2026-10-20T00:00:00Z</date>
<paths><path action="A" kind="dir" copyfrom-path="/trunk" copyfrom-rev="2">/branches/feat</path></paths>
<msg>多行
信息</msg></logentry></log>`);
    expect(log).toEqual([
      {
        revision: 3,
        author: "bob",
        date: "2026-10-20T00:00:00Z",
        message: "多行\n信息",
        changedPaths: [
          {
            action: "A",
            path: "/branches/feat",
            kind: "dir",
            copyFromPath: "/trunk",
            copyFromRevision: 2,
          },
        ],
      },
    ]);
    expect(
      parseSvnListXml(
        `<lists><list path="x"><entry kind="dir"><name>feat</name></entry><entry kind="file"><name>README</name></entry></list></lists>`,
      ),
    ).toEqual([
      { name: "feat", kind: "dir" },
      { name: "README", kind: "file" },
    ]);
  });

  it("统计 diff 行数时跳过文件头与属性差异，并识别二进制", () => {
    const patch = [
      "Index: a.txt",
      "===================================================================",
      "--- a.txt\t(revision 1)",
      "+++ a.txt\t(working copy)",
      "@@ -1,2 +1,2 @@",
      "--- removed line that looks like a header",
      "+added",
      " context",
      "Property changes on: a.txt",
      "___________________________________________________________________",
      "Added: svn:eol-style",
      "## -0,0 +1 ##",
      "+native",
    ].join("\n");
    expect(countSvnDiffLines(patch)).toEqual({ additions: 1, deletions: 1, isBinary: false });
    expect(
      countSvnDiffLines(
        "Index: b.png\n===\nCannot display: file marked as a binary type.\nsvn:mime-type = application/octet-stream\n",
      ).isBinary,
    ).toBe(true);
  });

  it("解析 update 变更行与 commit 修订号", () => {
    expect(
      countSvnUpdatedPaths(
        "Updating '.':\nU    a.txt\n   C 中文\nA    new/file.txt\nUpdated to revision 4.\nSummary of conflicts:\n  Text conflicts: 1\n",
      ),
    ).toBe(3);
    expect(
      parseSvnCommittedRevision(
        "Sending a.txt\nTransmitting file data .done\nCommitting transaction...\nCommitted revision 12.\n",
      ),
    ).toBe(12);
    expect(parseSvnCommittedRevision("")).toBeNull();
  });
});

describe("svn 错误归类", () => {
  it("只按错误码归类", () => {
    expect(classifySvnError("svn: E155004: Working copy '/wc' locked.")).toBe(
      "working-copy-locked",
    );
    expect(classifySvnError("svn: E155011: File '/wc/a' is out of date")).toBe("out-of-date");
    expect(classifySvnError("svn: E170001: Authentication required")).toBe("auth-required");
    expect(classifySvnError("svn: E155015: Aborting commit: remains in conflict")).toBe(
      "unresolved-conflicts",
    );
    expect(classifySvnError("svn: E155007: '/x' is not a working copy")).toBe("not-working-copy");
    // E170013 单独出现表示无法连接服务器，不应误报为认证问题
    expect(classifySvnError("svn: E170013: Unable to connect to a repository")).toBe("unknown");
    expect(
      classifySvnError("svn: E170013: Unable to connect\nsvn: E215004: No more credentials"),
    ).toBe("auth-required");
  });

  it("摘要只保留错误码行", () => {
    expect(summarizeSvnStderr("noise\nsvn: E155004: locked\n\n")).toBe("svn: E155004: locked");
  });
});

describe("svn 环境与版本", () => {
  it("保持 UTF-8 字符集、消息固定英文并移除编辑器变量", () => {
    const env = getSvnCommandEnv(
      { LC_ALL: "zh_CN.UTF-8", EDITOR: "vim", SVN_SSH: "ssh -i key", PATH: "/bin" },
      "linux",
    );
    expect(env.LC_ALL).toBeUndefined();
    expect(env.LC_CTYPE).toBe("zh_CN.UTF-8");
    expect(env.LC_MESSAGES).toBe("C");
    expect(env.EDITOR).toBeUndefined();
    expect(env.SVN_EDITOR).toBe("false");
    expect(env.SVN_SSH).toBe("ssh -i key");
    expect(getSvnCommandEnv({ LANG: "C" }, "linux").LC_CTYPE).toBe("C.UTF-8");
    expect(getSvnCommandEnv({ LANG: "C" }, "darwin").LC_CTYPE).toBe("en_US.UTF-8");
    expect(getSvnCommandEnv({ LANG: "C" }, "win32").LC_CTYPE).toBeUndefined();
  });

  it("比较版本号", () => {
    expect(compareSvnVersions("1.14.3", "1.8.0")).toBeGreaterThan(0);
    expect(compareSvnVersions("1.7.22", "1.8.0")).toBeLessThan(0);
    expect(compareSvnVersions("1.10.0", "1.10.0")).toBe(0);
  });
});

describe("svn 分支推导", () => {
  it("只识别标准布局", () => {
    expect(deriveSvnBranchRef("trunk")).toEqual({
      kind: "trunk",
      name: "trunk",
      relativePath: "trunk",
    });
    expect(deriveSvnBranchRef("trunk/src")).toMatchObject({ kind: "trunk" });
    expect(deriveSvnBranchRef("branches/feat/src")).toEqual({
      kind: "branch",
      name: "feat",
      relativePath: "branches/feat",
    });
    expect(deriveSvnBranchRef("tags/v1")).toMatchObject({ kind: "tag", name: "v1" });
    expect(deriveSvnBranchRef("project/main")).toEqual({
      kind: "other",
      name: "project/main",
      relativePath: "project/main",
    });
    expect(deriveSvnBranchRef("")).toMatchObject({ kind: "other", name: "/" });
  });

  it("计算并解码相对仓库路径", () => {
    expect(getSvnRelativeRepositoryPath("file:///r/branches/%E7%89%B9", "file:///r")).toBe(
      "branches/特",
    );
    expect(getSvnRelativeRepositoryPath("file:///other/x", "file:///r")).toBeNull();
  });
});
