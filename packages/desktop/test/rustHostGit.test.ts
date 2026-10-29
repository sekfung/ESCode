import { describe, expect, it } from "vitest";
import { execFileSync } from "node:child_process";
import { existsSync } from "node:fs";
import { mkdir, mkdtemp, readFile, realpath, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { connectRpc, hostBinary, nextMeaningful, startHost } from "./rustHostTestSupport.js";

// Phase 3 golden freeze：原「真实 TS `createGitService`」oracle 的逐步观察已冻结为仓内资产
// （packages/desktop/test/fixtures/golden/rustHostGit.oracle.json）。测试不再 import `@escode/services`，
// 删除 TS 实现后仍能独立重验 Rust Host 的 `git` channel（真实 `@escode/client`）。
const goldenFile = new URL("./fixtures/golden/rustHostGit.oracle.json", import.meta.url);

// docs/specs/rust-u2-git.md §5.1 TS oracle 差分：同一套夹具脚本在 Rust Host 的 `git` channel 上重放，
// 与冻结的 TS 观察逐字段全等。写操作改变仓库状态，所以夹具只建一份、按同一脚本顺序执行。

// 固定作者/提交时间与全局配置：录制时与重放时的提交哈希、时间戳一致。
const FIXED_ENV = {
  GIT_AUTHOR_DATE: "2026-01-01T00:00:00Z",
  GIT_COMMITTER_DATE: "2026-01-01T00:00:00Z",
  GIT_CONFIG_NOSYSTEM: "1",
};

function git(cwd: string, ...args: string[]): string {
  return execFileSync("git", args, { cwd, env: process.env, encoding: "utf8", stdio: "pipe" });
}

async function write(path: string, content: string | Buffer): Promise<void> {
  await mkdir(join(path, ".."), { recursive: true });
  await writeFile(path, content);
}

interface Fixture {
  plain: string;
  empty: string;
  main: string;
  conflict: string;
  worktree: string;
}

/** 构建一份夹具；与生成脚本 generate-rusthost-git-golden.ts 使用同一过程。 */
async function buildFixture(root: string): Promise<Fixture> {
  const plain = join(root, "plain");
  await mkdir(plain, { recursive: true });
  await writeFile(join(plain, "file.txt"), "x\n");

  // 空仓库：无提交 + 一个未跟踪文件。
  const empty = join(root, "empty");
  await mkdir(empty, { recursive: true });
  git(empty, "init", "-q", "-b", "main");
  await writeFile(join(empty, "hello.txt"), "one\ntwo");

  // 主仓库：workspace 是子目录 packages/app（monorepo 作用域），带 upstream、ahead/behind、多种改动。
  const remote = join(root, "remote.git");
  git(root, "init", "-q", "--bare", "-b", "main", remote);
  const repo = join(root, "repo");
  await mkdir(repo, { recursive: true });
  git(repo, "init", "-q", "-b", "main");
  const app = join(repo, "packages", "app");
  await write(join(app, "a.txt"), "1\n2\n3\n");
  await write(join(app, "del.txt"), "delete me\n");
  await write(join(app, "old.txt"), "rename me\nkeep\n");
  await write(join(app, "img.bin"), Buffer.from([0, 1, 2, 3, 0, 5]));
  await write(join(app, ".gitignore"), "*.log\n");
  await write(join(repo, "README.md"), "root\n");
  git(repo, "add", "-A");
  git(repo, "commit", "-q", "-m", "init");
  git(repo, "remote", "add", "origin", remote);
  git(repo, "push", "-q", "-u", "origin", "main");
  git(repo, "branch", "feature");
  git(repo, "switch", "-q", "feature");
  await writeFile(join(app, "a.txt"), "1\nfeature\n3\n");
  git(repo, "commit", "-q", "-am", "feature change");
  git(repo, "switch", "-q", "main");
  // 远端领先一条（behind=1）。
  const other = join(root, "other");
  git(root, "clone", "-q", remote, other);
  await writeFile(join(other, "remote.txt"), "remote\n");
  git(other, "add", "-A");
  git(other, "commit", "-q", "-m", "remote change");
  git(other, "push", "-q", "origin", "main");
  git(repo, "fetch", "-q", "origin");
  // 本地领先一条（ahead=1）。
  await writeFile(join(app, "local.txt"), "local\n");
  git(repo, "add", "-A");
  git(repo, "commit", "-q", "-m", "local change");
  // 工作区：未暂存修改、已暂存重命名、删除、二进制修改、未跟踪、忽略、作用域外修改、额外已暂存文件。
  await writeFile(join(app, "a.txt"), "1\n2\n3\nmore\n");
  git(app, "mv", "old.txt", "new.txt");
  await rm(join(app, "del.txt"));
  await writeFile(join(app, "img.bin"), Buffer.from([0, 9, 9, 0]));
  await writeFile(join(app, "untracked.txt"), "u1\nu2\nu3\n");
  await writeFile(join(app, "debug.log"), "ignored\n");
  await writeFile(join(repo, "README.md"), "root changed\n");
  await writeFile(join(app, "staged-only.txt"), "s\n");
  git(app, "add", "staged-only.txt");
  // 链接工作树：getWorkspaceRepositoryInfo 的 linked-worktree 分支。
  const worktree = join(root, "wt");
  git(repo, "worktree", "add", "-q", "-b", "wt-branch", worktree, "feature");

  // 冲突仓库：合并冲突进行中。
  const conflict = join(root, "conflict");
  await mkdir(conflict, { recursive: true });
  git(conflict, "init", "-q", "-b", "main");
  await writeFile(join(conflict, "c.txt"), "base\n");
  git(conflict, "add", "-A");
  git(conflict, "commit", "-q", "-m", "base");
  git(conflict, "switch", "-q", "-c", "other");
  await writeFile(join(conflict, "c.txt"), "other\n");
  git(conflict, "commit", "-q", "-am", "other");
  git(conflict, "switch", "-q", "main");
  await writeFile(join(conflict, "c.txt"), "main\n");
  git(conflict, "commit", "-q", "-am", "main");
  try {
    git(conflict, "merge", "-q", "other");
  } catch {
    // 预期冲突。
  }

  return { plain, empty, main: app, conflict, worktree };
}

type GitService = Awaited<ReturnType<typeof connectRpc>>["gitService"];
type Step = (git: GitService, fixture: Fixture) => Promise<unknown>;

/** 成功值或错误文案都参与比较；void 方法：TS 冻结为 `<undefined>`，Rust 必须回 `Wire::Undefined`。 */
async function settle(step: Step, service: GitService, fixture: Fixture): Promise<unknown> {
  try {
    const value = await step(service, fixture);
    return { ok: value === undefined ? "<undefined>" : value };
  } catch (error) {
    return { error: error instanceof Error ? error.message : String(error) };
  }
}

const STEPS: Array<[string, Step]> = [
  // 非仓库目录。
  ["plain summary", (g, f) => g.getRepositorySummary({ workspacePath: f.plain })],
  ["plain info", (g, f) => g.getWorkspaceRepositoryInfo({ workspacePath: f.plain })],
  ["plain branches", (g, f) => g.getLocalBranches({ workspacePath: f.plain })],
  ["plain graph", (g, f) => g.getCommitGraph({ workspacePath: f.plain })],
  ["plain changes", (g, f) => g.getChanges({ workspacePath: f.plain, sourceId: "unstaged" })],
  ["plain diff", (g, f) => g.getDiff({ workspacePath: f.plain, path: "file.txt" })],
  ["plain identity", (g, f) => g.getIdentity({ workspacePath: f.plain })],
  ["plain comparison", (g, f) => g.getBranchComparison({ workspacePath: f.plain })],
  ["plain stage", (g, f) => g.stagePaths({ workspacePath: f.plain, paths: ["file.txt"] })],
  ["plain commit", (g, f) => g.commit({ workspacePath: f.plain, message: "m" })],
  ["plain push", (g, f) => g.push({ workspacePath: f.plain })],
  ["plain switch", (g, f) => g.switchBranch({ workspacePath: f.plain, targetBranchName: "x" })],
  // 空仓库。
  ["empty summary", (g, f) => g.getRepositorySummary({ workspacePath: f.empty })],
  ["empty branches", (g, f) => g.getLocalBranches({ workspacePath: f.empty })],
  ["empty graph", (g, f) => g.getCommitGraph({ workspacePath: f.empty })],
  ["empty changes", (g, f) => g.getChanges({ workspacePath: f.empty, sourceId: "unstaged" })],
  ["empty comparison", (g, f) => g.getBranchComparison({ workspacePath: f.empty })],
  [
    "empty untracked diff",
    (g, f) => g.getDiff({ workspacePath: f.empty, path: "hello.txt", sourceId: "unstaged" }),
  ],
  [
    "empty refresh",
    (g, f) =>
      g.refresh({ workspacePath: f.empty, includeIdentity: true, includeBranchComparison: true }),
  ],
  // 主仓库（子目录 workspace）读面。
  ["main summary", (g, f) => g.getRepositorySummary({ workspacePath: f.main })],
  ["main info", (g, f) => g.getWorkspaceRepositoryInfo({ workspacePath: f.main })],
  ["worktree info", (g, f) => g.getWorkspaceRepositoryInfo({ workspacePath: f.worktree })],
  ["main branches", (g, f) => g.getLocalBranches({ workspacePath: f.main })],
  ["main graph", (g, f) => g.getCommitGraph({ workspacePath: f.main })],
  ["main graph page", (g, f) => g.getCommitGraph({ workspacePath: f.main, maxCount: 2, skip: 1 })],
  ["main unstaged", (g, f) => g.getChanges({ workspacePath: f.main, sourceId: "unstaged" })],
  ["main staged", (g, f) => g.getChanges({ workspacePath: f.main, sourceId: "staged" })],
  [
    "main ignored",
    (g, f) =>
      g.getIgnoredPaths({
        workspacePath: f.main,
        paths: ["debug.log", "a.txt", "missing.log", "../../README.md"],
      }),
  ],
  ["main comparison", (g, f) => g.getBranchComparison({ workspacePath: f.main })],
  ["main identity", (g, f) => g.getIdentity({ workspacePath: f.main })],
  [
    "main refresh",
    (g, f) =>
      g.refresh({ workspacePath: f.main, includeIdentity: true, includeBranchComparison: true }),
  ],
  ["main refresh bare", (g, f) => g.refresh({ workspacePath: f.main })],
  ...(
    [
      ["a.txt", "unstaged"],
      ["a.txt", "branch"],
      ["new.txt", "staged"],
      ["del.txt", "unstaged"],
      ["img.bin", "unstaged"],
      ["untracked.txt", "unstaged"],
      ["staged-only.txt", "staged"],
      ["missing.txt", "unstaged"],
    ] as const
  ).map(([path, sourceId]): [string, Step] => [
    `main diff ${path} ${sourceId}`,
    (g, f) => g.getDiff({ workspacePath: f.main, path, sourceId }),
  ]),
  // 分支写面：失败归类与无变化成功。
  [
    "switch overwrite",
    (g, f) => g.switchBranch({ workspacePath: f.main, targetBranchName: "feature" }),
  ],
  ["switch same", (g, f) => g.switchBranch({ workspacePath: f.main, targetBranchName: " main " })],
  ["switch missing", (g, f) => g.switchBranch({ workspacePath: f.main, targetBranchName: "nope" })],
  ["switch blank", (g, f) => g.switchBranch({ workspacePath: f.main, targetBranchName: "  " })],
  [
    "create invalid",
    (g, f) => g.createBranchAndSwitch({ workspacePath: f.main, branchName: "bad..name" }),
  ],
  [
    "create exists",
    (g, f) => g.createBranchAndSwitch({ workspacePath: f.main, branchName: "main" }),
  ],
  [
    "create option start",
    (g, f) =>
      g.createBranchAndSwitch({ workspacePath: f.main, branchName: "topic0", startPoint: "-x" }),
  ],
  [
    "switch worktree branch",
    (g, f) => g.switchBranch({ workspacePath: f.main, targetBranchName: "wt-branch" }),
  ],
  // 路径写面。
  ["stage dup", (g, f) => g.stagePaths({ workspacePath: f.main, paths: ["a.txt", "./a.txt"] })],
  ["stage outside", (g, f) => g.stagePaths({ workspacePath: f.main, paths: ["../../../x"] })],
  ["unstage", (g, f) => g.unstagePaths({ workspacePath: f.main, paths: ["staged-only.txt"] })],
  ["discard", (g, f) => g.discardPaths({ workspacePath: f.main, paths: ["del.txt"] })],
  ["after paths", (g, f) => g.getChanges({ workspacePath: f.main, sourceId: "staged" })],
  ["after paths unstaged", (g, f) => g.getChanges({ workspacePath: f.main, sourceId: "unstaged" })],
  // 提交：空消息、只提交选中已暂存路径、全部提交。
  ["commit blank", (g, f) => g.commit({ workspacePath: f.main, message: "   " })],
  ["stage more", (g, f) => g.stagePaths({ workspacePath: f.main, paths: ["staged-only.txt"] })],
  // 已暂存后再改工作区：stagedOnly 必须提交已暂存版本（`commit -- path` 会误取工作区版本）。
  [
    "edit after stage",
    (_g, f) => writeFile(join(f.main, "a.txt"), "1\n2\n3\nmore\nworktree-only\n"),
  ],
  [
    "commit staged only",
    (g, f) =>
      g.commit({ workspacePath: f.main, message: "partial", paths: ["a.txt"], stagedOnly: true }),
  ],
  ["after partial", (g, f) => g.getChanges({ workspacePath: f.main, sourceId: "staged" })],
  [
    "after partial unstaged",
    (g, f) => g.getChanges({ workspacePath: f.main, sourceId: "unstaged" }),
  ],
  ["commit all", (g, f) => g.commit({ workspacePath: f.main, message: " all \n" })],
  ["after commit", (g, f) => g.refresh({ workspacePath: f.main, includeBranchComparison: true })],
  // 推送：落后远端被拒；新分支 set-upstream 成功。
  ["push rejected", (g, f) => g.push({ workspacePath: f.main })],
  [
    "create topic",
    (g, f) => g.createBranchAndSwitch({ workspacePath: f.main, branchName: "topic" }),
  ],
  ["push upstream", (g, f) => g.push({ workspacePath: f.main })],
  ["push again", (g, f) => g.push({ workspacePath: f.main })],
  // 冲突仓库。
  [
    "conflict unstaged",
    (g, f) => g.getChanges({ workspacePath: f.conflict, sourceId: "unstaged" }),
  ],
  ["conflict staged", (g, f) => g.getChanges({ workspacePath: f.conflict, sourceId: "staged" })],
  [
    "conflict switch",
    (g, f) => g.switchBranch({ workspacePath: f.conflict, targetBranchName: "other" }),
  ],
  [
    "conflict create",
    (g, f) => g.createBranchAndSwitch({ workspacePath: f.conflict, branchName: "x" }),
  ],
  ["conflict resolve", (g, f) => g.stagePaths({ workspacePath: f.conflict, paths: ["c.txt"] })],
  [
    "merge in progress switch",
    (g, f) => g.switchBranch({ workspacePath: f.conflict, targetBranchName: "other" }),
  ],
];

describe("rust host git domain", () => {
  it.skipIf(!existsSync(hostBinary))(
    "matches the frozen TS git service observations",
    async () => {
      const base = await realpath(await mkdtemp(join(tmpdir(), "escode-rust-host-git-")));
      const saved = { ...process.env };
      const globalConfig = join(base, "gitconfig");
      await writeFile(
        globalConfig,
        '[user]\n\tname = Fixture User\n\temail = fixture@example.invalid\n[protocol "file"]\n\tallow = always\n',
      );
      Object.assign(process.env, FIXED_ENV, { GIT_CONFIG_GLOBAL: globalConfig });
      const home = join(base, "home");
      const host = startHost(home);
      try {
        // 夹具与全局 gitconfig 同在 base 下：冻结里的 `$FIXTURE_ROOT/gitconfig` 可直接替换。
        const root = base;
        const fixture = await buildFixture(root);

        expect((await nextMeaningful(host)).kind).toBe("hostReady");
        host.send({ kind: "rpcListen", bind: "127.0.0.1:0", home });
        const listening = await nextMeaningful(host);
        expect(listening.kind).toBe("rpcListening");
        const services = await connectRpc(String(listening.addr), String(listening.secret));
        const rustGit = services.gitService;

        // 冻结观察中的绝对临时路径统一写成 $FIXTURE_ROOT；替换为本次夹具根后逐字段比较。
        const golden = JSON.parse(
          (await readFile(goldenFile, "utf8")).split("$FIXTURE_ROOT").join(root),
        ) as { expected: Record<string, unknown> };
        // 冻结必须覆盖脚本的每一步，避免后续新增场景时静默漏测。
        expect(Object.keys(golden.expected)).toEqual(STEPS.map(([label]) => label));

        const dropRemoteHead = (value: unknown): unknown => {
          if (Array.isArray(value)) return value.map(dropRemoteHead);
          if (!value || typeof value !== "object") return value;
          return Object.fromEntries(
            Object.entries(value).flatMap(([key, entry]) => {
              if (key !== "refs" || !Array.isArray(entry)) return [[key, dropRemoteHead(entry)]];
              return [
                [
                  key,
                  entry
                    .filter(
                      (ref) =>
                        !(
                          ref &&
                          typeof ref === "object" &&
                          (ref as { name?: unknown }).name === "origin/HEAD"
                        ),
                    )
                    .map(dropRemoteHead),
                ],
              ];
            }),
          );
        };
        for (const [label, step] of STEPS) {
          const actual = await settle(step, rustGit, fixture);
          expect({ label, result: dropRemoteHead(actual) }).toEqual({
            label,
            result: dropRemoteHead(golden.expected[label]),
          });
        }

        // generateCommitMessage 的模型编排见 rustHostCommitMessage.test.ts；这里的 Host 没有配 provider 文件，
        // 有变更的仓库会在“读取当前模型”这一步失败（git 侧输入已先于它通过）。
        await expect(
          rustGit.generateCommitMessage({ workspacePath: fixture.main } as never),
        ).rejects.toThrow(/未读取到当前模型/);
      } finally {
        await host.stop();
        process.env = saved;
        await rm(base, { recursive: true, force: true });
      }
    },
    120_000,
  );
});
