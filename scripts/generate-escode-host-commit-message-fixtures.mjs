// Run with node --import tsx. 用**真实 TS 生成器**（packages/services/src/git/gitCommitMessageGenerator.ts）
// 对一组输入导出 prompt 与输出校验结果，供 Rust 侧 `commit_message_prompt` 对拍
// （docs/specs/rust-u2-git.md §7）。prompt 文本与校验规则是模型行为的一部分，必须逐字一致。
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { dirname } from "node:path";

import { GitCommitMessageGenerator } from "../packages/services/src/git/gitCommitMessageGenerator.ts";

const FIXTURE_VERSION = 1;
const OUT = "apps/escode-host/crates/host-services/src/git/commit_message_golden.json";

const file = (i, kind = "modified", added = i, removed = 0) => ({
  path: `/repo/src/f${i}.ts`,
  repoRelativePath: `src/f${i}.ts`,
  workspaceRelativePath: `src/f${i}.ts`,
  kind,
  section: "unstaged",
  added,
  removed,
  isStaged: false,
  isUntracked: false,
  isConflicted: false,
});
const diff = (path, patch, summary) => ({
  path,
  ...(patch === undefined ? {} : { patch }),
  ...(summary === undefined ? {} : { summary }),
});
const long = (n, ch = "x") => ch.repeat(n);

const promptCases = [
  {
    name: "minimal",
    branchName: "main",
    locale: "en-US",
    files: [file(1)],
    diffs: [],
    conversationContext: undefined,
  },
  {
    name: "chinese-no-branch",
    branchName: null,
    locale: "zh-CN",
    files: [file(1, "added", 3, 0)],
    diffs: [diff("a", "@@ -1 +1 @@\n-a\n+b")],
  },
  {
    name: "branch-whitespace",
    branchName: "  feature/x  ",
    locale: "en-US",
    files: [file(2)],
    diffs: [],
  },
  {
    name: "many-files",
    branchName: "main",
    locale: "en-US",
    files: Array.from({ length: 27 }, (_, i) => file(i, "modified", i, i + 1)),
    diffs: [],
  },
  {
    name: "diff-clipping",
    branchName: "main",
    locale: "en-US",
    files: [file(1)],
    diffs: [
      diff("big1", long(5000)),
      diff("summary-only", undefined, "  binary file changed  "),
      diff("blank", "   ", "  "),
      ...Array.from({ length: 9 }, (_, i) => diff(`d${i}`, long(1900, String(i)))),
    ],
  },
  {
    name: "conversation",
    branchName: "main",
    locale: "zh-CN",
    files: [file(1)],
    diffs: [],
    conversationContext: {
      omittedMessageCount: 3,
      messages: [
        { role: "user", content: "  请帮我\r\n\r\n\r\n\r\n修复登录   \t\n问题  " },
        { role: "assistant", content: long(900, "好") },
        { role: "user", content: "   " },
        ...Array.from({ length: 14 }, (_, i) => ({
          role: i % 2 ? "assistant" : "user",
          content: `message ${i} ${long(400, "m")}`,
        })),
      ],
    },
  },
  {
    name: "conversation-total-clip",
    branchName: "main",
    locale: "en-US",
    files: [file(1)],
    diffs: [],
    conversationContext: {
      omittedMessageCount: 0,
      messages: Array.from({ length: 12 }, (_, i) => ({
        role: "user",
        content: `${i}:${long(590, "q")}`,
      })),
    },
  },
  {
    name: "astral-clip",
    branchName: "main",
    locale: "en-US",
    files: [file(1)],
    diffs: [diff("emoji", `x${"😀".repeat(1500)}`)],
  },
];

// 注：JS 在 UTF-16 码元边界截断会产生孤立代理项，JSON/Rust 都无法表示；用例特意让截断点落在代理对边界上。
const outputs = [
  "feat: add x",
  "fix(core): handle null",
  "refactor(a-b)!: rename",
  "```\nfeat: fenced\n```",
  "```text\nfix: fenced with lang\n\nbody line\n```",
  '"feat: quoted"',
  "'fix: single'",
  "Commit message: chore: prefixed",
  "commit message:   docs: lower prefix",
  "  feat: padded  \n\nBody here\n",
  "Here is your commit message:\nfeat: a",
  "feature: not a type",
  "feat:no space",
  "feat(): empty scope",
  "feat: " + long(100),
  "feat: " + long(101),
  "feat: " + "😀".repeat(50),
  "feat: " + "😀".repeat(51),
  "feat: ok\r\nbody after crlf",
  "```\n\n```",
  "   ",
  "feat: " + long(1200),
  "Revert: Wrong case",
  "revert: this reverts",
];

async function generatePrompt(testCase) {
  let captured;
  const generator = new GitCommitMessageGenerator({
    currentModelProvider: { readCurrentModel: async () => ({ providerId: "p", modelId: "m" }) },
    textGenerator: {
      generateText: async (params) => {
        captured = params.prompt;
        return { text: "feat: x", selection: params.selection };
      },
    },
  });
  await generator.generate({
    workspacePath: "/w",
    branchName: testCase.branchName,
    locale: testCase.locale,
    files: testCase.files,
    diffs: testCase.diffs,
    conversationContext: testCase.conversationContext,
  });
  return captured;
}

async function generateValidation(text) {
  const generator = new GitCommitMessageGenerator({
    currentModelProvider: { readCurrentModel: async () => ({ providerId: "p", modelId: "m" }) },
    textGenerator: { generateText: async (params) => ({ text, selection: params.selection }) },
  });
  try {
    const result = await generator.generate({
      workspacePath: "/w",
      branchName: "main",
      locale: "en-US",
      files: [file(1)],
      diffs: [],
    });
    return { ok: true, message: result.message };
  } catch (error) {
    return {
      ok: false,
      message: error.message,
      reason: error.reason,
      detail: error.detail ?? null,
    };
  }
}

async function main() {
  const prompts = [];
  for (const testCase of promptCases) {
    prompts.push({ ...testCase, prompt: await generatePrompt(testCase) });
  }
  const validations = [];
  for (const text of outputs) validations.push({ text, ...(await generateValidation(text)) });
  const text = `${JSON.stringify({ fixtureVersion: FIXTURE_VERSION, prompts, validations }, null, 2)}\n`;
  if (process.argv.includes("--check")) {
    const current = await readFile(OUT, "utf8").catch(() => null);
    if (current !== text) {
      console.error(
        `[commit-message] 期望资产已过期：${OUT}\n运行 node --import tsx ${process.argv[1]} 重新生成`,
      );
      process.exit(1);
    }
    console.log(`[commit-message] 期望资产一致：${OUT}`);
    return;
  }
  await mkdir(dirname(OUT), { recursive: true });
  await writeFile(OUT, text);
  process.stdout.write(
    `commit message golden: ${prompts.length} prompts, ${validations.length} validations → ${OUT}\n`,
  );
}

await main();
