// Run with node --import tsx. 以 TS isGitRuntimeContextUnsafe 为 oracle，导出 git 上下文安全判定语料。
// 每个用例描述一棵目录树（相对路径 + 种类 + 内容/链接目标）与查询 cwd，Rust 端重建同构目录后逐条比对。
import { readFile, writeFile, mkdtemp, mkdir, rm, symlink, writeFile as write } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { isGitRuntimeContextUnsafe } from "../apps/zcode-cli/packages/core/src/tool/handlers/bash-git-runtime-safety.ts";

const HEAD = "ref: refs/heads/main\n";
const OBJECT_ID = "a".repeat(40) + "\n";

/** kind: dir | file | symlink */
const cases = [
  { name: "plain", cwd: "plain", entries: [["plain/sub", "dir", ""]], expectAncestors: true },
  {
    name: "trusted-directory",
    cwd: "trusted",
    entries: [
      ["trusted/.git/HEAD", "file", HEAD],
      ["trusted/.git/objects", "dir", ""],
      ["trusted/.git/refs", "dir", ""],
    ],
    expect: false,
  },
  {
    name: "trusted-parent",
    cwd: "trusted-parent/work",
    entries: [
      ["trusted-parent/work", "dir", ""],
      ["trusted-parent/.git/HEAD", "file", HEAD],
      ["trusted-parent/.git/objects", "dir", ""],
      ["trusted-parent/.git/refs", "dir", ""],
    ],
    expect: false,
  },
  {
    // cwd 就是 .git 目录本身：HEAD 文件即裸仓库标识，必须判为不安全。
    name: "cwd-is-dot-git",
    cwd: "cwdgit/.git",
    entries: [
      ["cwdgit/.git/HEAD", "file", HEAD],
      ["cwdgit/.git/objects", "dir", ""],
      ["cwdgit/.git/refs", "dir", ""],
    ],
    expect: true,
  },
  {
    name: "gitfile-outside-relative",
    cwd: "gitfile/work",
    entries: [
      ["gitfile/work", "dir", ""],
      ["gitfile/work/.git", "file", "gitdir: ../real/.git\n"],
      ["gitfile/real/.git/HEAD", "file", OBJECT_ID],
      ["gitfile/real/.git/objects", "dir", ""],
      ["gitfile/real/.git/refs", "dir", ""],
    ],
    expect: false,
  },
  {
    name: "gitfile-inside-cwd",
    cwd: "inside/work",
    entries: [
      ["inside/work", "dir", ""],
      ["inside/work/inner/.git/HEAD", "file", HEAD],
      ["inside/work/inner/.git/objects", "dir", ""],
      ["inside/work/inner/.git/refs", "dir", ""],
      ["inside/work/.git", "file", "gitdir: ./inner/.git\n"],
    ],
    expect: true,
  },
  {
    name: "gitfile-target-without-git-segment",
    cwd: "nogit/work",
    entries: [
      ["nogit/work", "dir", ""],
      ["nogit/work/.git", "file", "gitdir: ../elsewhere\n"],
      ["nogit/elsewhere/HEAD", "file", HEAD],
    ],
    expect: true,
  },
  {
    name: "gitfile-target-invalid-head",
    cwd: "invalid/work",
    entries: [
      ["invalid/work", "dir", ""],
      ["invalid/work/.git", "file", "gitdir: ../real/.git\n"],
      ["invalid/real/.git/HEAD", "file", "not-a-ref\n"],
      ["bare-parent/objects", "dir", ""],
    ],
    expectAncestors: true,
  },
  {
    name: "bare-indicators-cwd",
    cwd: "bare",
    entries: [
      ["bare/HEAD", "file", HEAD],
      ["bare/objects", "dir", ""],
      ["bare/refs", "dir", ""],
    ],
    expect: true,
  },
  {
    name: "bare-indicators-parent",
    cwd: "bare-parent/work",
    entries: [
      ["bare-parent/work", "dir", ""],
      ["bare-parent/HEAD", "file", HEAD],
      ["bare-parent/objects", "dir", ""],
    ],
    expect: true,
  },
  {
    name: "gitfile-not-a-gitdir-line",
    cwd: "notgitdir/work",
    entries: [
      ["notgitdir/work", "dir", ""],
      ["notgitdir/work/.git", "file", "something else\n"],
    ],
    expectAncestors: true,
  },
  {
    name: "gitfile-with-nul",
    cwd: "nul/work",
    entries: [
      ["nul/work", "dir", ""],
      ["nul/work/.git", "file", "gitdir: ../real/.git\0\n"],
    ],
    expect: true,
  },
  {
    name: "gitfile-oversized",
    cwd: "big/work",
    entries: [
      ["big/work", "dir", ""],
      ["big/work/.git", "file", `gitdir: ../real/.git\n${"x".repeat(33 * 1024)}`],
    ],
    expect: true,
  },
  {
    name: "symlink-dot-git-outside",
    cwd: "link/work",
    entries: [
      ["link/work", "dir", ""],
      ["link/real/.git/HEAD", "file", HEAD],
      ["link/real/.git/objects", "dir", ""],
      ["link/real/.git/refs", "dir", ""],
      ["link/work/.git", "symlink", "../real/.git"],
    ],
    expect: false,
  },
  {
    name: "commondir-present",
    cwd: "common/work",
    entries: [
      ["common/work", "dir", ""],
      ["common/work/.git/HEAD", "file", HEAD],
      ["common/work/.git/objects", "dir", ""],
      ["common/work/.git/refs", "dir", ""],
      ["common/work/.git/commondir", "file", "../main\n"],
    ],
    expectAncestors: true,
  },
  {
    name: "objects-missing",
    cwd: "noobjects/work",
    entries: [
      ["noobjects/work", "dir", ""],
      ["noobjects/work/.git/HEAD", "file", HEAD],
      ["noobjects/work/.git/refs", "dir", ""],
    ],
    expectAncestors: true,
  },
];

const root = await mkdtemp(join(tmpdir(), "zcode-git-safety-"));
const corpus = [];
try {
  for (const testCase of cases) {
    const dir = join(root, testCase.name);
    await mkdir(dir, { recursive: true });
    for (const [path, kind, value] of testCase.entries) {
      const target = join(dir, path);
      await mkdir(dirname(target), { recursive: true });
      if (kind === "dir") await mkdir(target, { recursive: true });
      else if (kind === "file") await write(target, value);
      else if (kind === "symlink") await symlink(value, target);
    }
    const cwd = join(dir, testCase.cwd);
    await mkdir(cwd, { recursive: true });
    const unsafe = isGitRuntimeContextUnsafe({ workingDirectory: cwd });
    corpus.push({
      name: testCase.name,
      cwd: testCase.cwd,
      entries: testCase.entries,
      // expectAncestors 的用例结论取决于本机 %TEMP% 之上是否存在仓库，不做跨机断言。
      expect: testCase.expectAncestors ? null : unsafe ? 1 : 0,
      unsafe,
    });
    console.log(testCase.name, unsafe);
  }
} finally {
  await rm(root, { recursive: true, force: true });
}

// symlink 用例在无权限的 Windows 上会抛错，届时用例缺席由 Rust 端按名跳过。
const content = `${JSON.stringify({ cases: corpus })}\n`;
const target = new URL(
  "../apps/zcode-cli-rust/crates/tools/tests/fixtures/git_safety_corpus.json",
  import.meta.url,
);
if (process.argv.includes("--check")) {
  if ((await readFile(target, "utf8")) !== content) {
    throw new Error(
      "Git safety corpus differs from TS; run node --import tsx scripts/generate-zcode-cli-rust-git-safety-corpus.mjs",
    );
  }
} else {
  await writeFile(target, content);
}
