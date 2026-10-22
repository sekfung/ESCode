import { describe, expect, it } from "vitest";
import { execFile, execFileSync } from "node:child_process";
import { existsSync } from "node:fs";
import { mkdir, mkdtemp, realpath, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { promisify } from "node:util";
import type { ISvnService } from "@escode/shared";
import { createSvnService } from "@escode/services/node";
import { connectRpc, hostBinary, nextMeaningful, startHost } from "./rustHostTestSupport.js";

// docs/svn/architecture.md「Rust Host」：同一套夹具建两份（A 给 TS `createSvnService()`，
// B 给 Rust Host 的 `svn` channel，经真实 `@escode/client`），按同一脚本逐步调用，
// 结果把各自根路径与提交时间归一后 `toEqual` 全等。写操作改变仓库状态，所以两边各用一份。
// 两个仓库用 `svnadmin hotcopy` 复制，UUID 与修订号一致，避免噪声。

const execFileAsync = promisify(execFile);
const svnBinary = process.env.ESCODE_SVN_BINARY?.trim() || "svn";
const svnadminBinary = process.env.ESCODE_SVNADMIN_BINARY?.trim() || "svnadmin";

function hasSvn(): boolean {
  try {
    execFileSync(svnBinary, ["--version", "--quiet"], { stdio: "ignore" });
    execFileSync(svnadminBinary, ["--version", "--quiet"], { stdio: "ignore" });
    return true;
  } catch {
    return false;
  }
}

async function svn(cwd: string, ...args: string[]): Promise<string> {
  const { stdout } = await execFileAsync(svnBinary, ["--non-interactive", ...args], {
    cwd,
    env: { ...process.env, LC_MESSAGES: "C" },
    encoding: "utf8",
  });
  return stdout;
}

async function write(path: string, content: string | Buffer): Promise<void> {
  await mkdir(join(path, ".."), { recursive: true });
  await writeFile(path, content);
}

interface Fixture {
  root: string;
  repo: string;
  repoUrl: string;
  wc: string;
  other: string;
  feature: string;
}

async function buildRepository(root: string): Promise<Fixture> {
  const repo = join(root, "repo");
  await mkdir(root, { recursive: true });
  await execFileAsync(svnadminBinary, ["create", repo]);
  const repoUrl = pathToFileURL(repo).href;
  await svn(root, "mkdir", "-m", "layout", `${repoUrl}/trunk`, `${repoUrl}/branches`);
  const wc = join(root, "wc");
  await svn(root, "checkout", "-q", `${repoUrl}/trunk`, wc);
  await write(join(wc, "tracked.md"), "line one\nline two\n");
  await write(join(wc, "del.txt"), "delete me\n");
  await write(join(wc, "img.bin"), Buffer.from([0, 1, 2, 3, 0, 5]));
  await write(join(wc, "ignored.log"), "ignored\n");
  // 属性与新增文件放在同一次提交，避免 WC 根目录在提交后仍停在上一修订导致后续 propset 提交过期。
  await svn(wc, "propset", "-q", "svn:ignore", "ignored.log", ".");
  await svn(wc, "add", "-q", "--parents", "tracked.md", "del.txt", "img.bin");
  await svn(wc, "commit", "-q", "-m", "test: baseline");
  await svn(root, "copy", "-m", "test: branch", `${repoUrl}/trunk`, `${repoUrl}/branches/feature`);
  // A 的 WC 在基线提交后停在上一修订；B 是 hotcopy 之后新 checkout 的 HEAD。先对齐修订号。
  await svn(wc, "update", "-q");
  const other = join(root, "other");
  await svn(root, "checkout", "-q", `${repoUrl}/trunk`, other);
  const feature = join(root, "feature");
  await svn(root, "checkout", "-q", `${repoUrl}/branches/feature`, feature);
  return { root, repo, repoUrl, wc, other, feature };
}

async function checkoutCopy(root: string): Promise<Fixture> {
  const repo = join(root, "repo");
  const repoUrl = pathToFileURL(repo).href;
  const wc = join(root, "wc");
  const other = join(root, "other");
  const feature = join(root, "feature");
  await svn(root, "checkout", "-q", `${repoUrl}/trunk`, wc);
  await svn(root, "checkout", "-q", `${repoUrl}/trunk`, other);
  await svn(root, "checkout", "-q", `${repoUrl}/branches/feature`, feature);
  return { root, repo, repoUrl, wc, other, feature };
}

async function modifyWorkingCopy(fixture: Fixture): Promise<void> {
  await writeFile(join(fixture.wc, "tracked.md"), "line one\nlocal change\n");
  await writeFile(join(fixture.wc, "new.txt"), "new file\nsecond\n");
  await writeFile(join(fixture.wc, "img.bin"), Buffer.from([0, 9, 9, 0]));
  await rm(join(fixture.wc, "del.txt"));
  await writeFile(join(fixture.wc, "ignored.log"), "ignored changed\n");
}

/** 另一个工作副本先提交同一行，制造 update 冲突。 */
async function prepareConflict(fixture: Fixture): Promise<void> {
  await writeFile(join(fixture.other, "tracked.md"), "line one\nincoming change\n");
  await svn(fixture.other, "commit", "-q", "-m", "test: incoming");
  await writeFile(join(fixture.wc, "tracked.md"), "line one\nlocal conflict\n");
}

type Step = (svn: ISvnService, fixture: Fixture) => Promise<unknown>;

async function settle(step: Step, service: ISvnService, fixture: Fixture): Promise<unknown> {
  try {
    const value = await step(service, fixture);
    // void 方法：TS 回 undefined，Rust 必须回 `Wire::Undefined`。
    return { ok: value === undefined ? "<undefined>" : value };
  } catch (error) {
    return { error: error instanceof Error ? error.message : String(error) };
  }
}

function normalize(value: unknown, root: string): unknown {
  const text = JSON.stringify(value)
    .split(root)
    .join("<ROOT>")
    // 提交时间必然不同（两个仓库交替提交），归一后再比较结构。
    .replace(/\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?Z/g, "<DATE>")
    // shelf 的创建时间取当前时间，归一后再比较。
    .replace(/"createdAt":\d+/g, '"createdAt":0');
  return JSON.parse(text);
}

const STEPS: Array<[string, Step]> = [
  ["info", (s, f) => s.getWorkingCopyInfo({ workspacePath: f.wc })],
  ["changes", (s, f) => s.getChanges({ workspacePath: f.wc })],
  ["diff tracked", (s, f) => s.getDiff({ workspacePath: f.wc, path: "tracked.md" })],
  ["diff new", (s, f) => s.getDiff({ workspacePath: f.wc, path: "new.txt" })],
  ["diff binary", (s, f) => s.getDiff({ workspacePath: f.wc, path: "img.bin" })],
  ["diff deleted", (s, f) => s.getDiff({ workspacePath: f.wc, path: "del.txt" })],
  ["diff ignored", (s, f) => s.getDiff({ workspacePath: f.wc, path: "ignored.log" })],
  ["diff unsafe", (s, f) => s.getDiff({ workspacePath: f.wc, path: "../escape.txt" })],
  ["remote status", (s, f) => s.getRemoteStatus({ workspacePath: f.wc })],
  ["log head", (s, f) => s.getLog({ workspacePath: f.wc, limit: 2 })],
  ["log page", (s, f) => s.getLog({ workspacePath: f.wc, limit: 2, startRevision: 1 })],
  ["branches", (s, f) => s.listBranches({ workspacePath: f.wc })],
  [
    "prepare conflict",
    async (_s, f) => {
      await prepareConflict(f);
      return "<prepared>";
    },
  ],
  ["update conflict", (s, f) => s.update({ workspacePath: f.wc })],
  ["changes conflict", (s, f) => s.getChanges({ workspacePath: f.wc })],
  ["diff conflict", (s, f) => s.getDiff({ workspacePath: f.wc, path: "tracked.md" })],
  [
    "commit conflict",
    (s, f) => s.commit({ workspacePath: f.wc, paths: ["tracked.md"], message: "test: conflict" }),
  ],
  [
    "resolve mine",
    (s, f) => s.resolve({ workspacePath: f.wc, path: "tracked.md", accept: "mine-full" }),
  ],
  ["changes resolved", (s, f) => s.getChanges({ workspacePath: f.wc })],
  [
    "commit selected",
    (s, f) =>
      s.commit({
        workspacePath: f.wc,
        paths: ["tracked.md", "new.txt", "del.txt", "img.bin"],
        message: "test: selected",
      }),
  ],
  ["changes committed", (s, f) => s.getChanges({ workspacePath: f.wc })],
  [
    "revert",
    async (s, f) => {
      await writeFile(join(f.wc, "tracked.md"), "line one\nrevert me\n");
      return s.revert({ workspacePath: f.wc, paths: ["tracked.md"] });
    },
  ],
  ["diff reverted", (s, f) => s.getDiff({ workspacePath: f.wc, path: "tracked.md" })],
  [
    "commit blank",
    (s, f) => s.commit({ workspacePath: f.wc, paths: ["tracked.md"], message: "  " }),
  ],
  ["commit empty", (s, f) => s.commit({ workspacePath: f.wc, paths: [], message: "x" })],
  ["commit unsafe", (s, f) => s.commit({ workspacePath: f.wc, paths: ["../x"], message: "x" })],
  ["revert unsafe", (s, f) => s.revert({ workspacePath: f.wc, paths: ["../x"] })],
  [
    "resolve invalid",
    (s, f) => s.resolve({ workspacePath: f.wc, path: "../x", accept: "working" }),
  ],
  [
    "switch feature",
    (s, f) => s.switchBranch({ workspacePath: f.wc, relativePath: "branches/feature" }),
  ],
  ["info feature", (s, f) => s.getWorkingCopyInfo({ workspacePath: f.wc })],
  ["log feature", (s, f) => s.getLog({ workspacePath: f.wc, limit: 2 })],
  ["switch trunk", (s, f) => s.switchBranch({ workspacePath: f.wc, relativePath: "trunk" })],
  ["switch invalid", (s, f) => s.switchBranch({ workspacePath: f.wc, relativePath: "../x" })],
  [
    "prepare feature change",
    async (_s, f) => {
      await writeFile(join(f.feature, "merged.md"), "from feature\n");
      await svn(f.feature, "add", "-q", "merged.md");
      await svn(f.feature, "commit", "-q", "-m", "test: feature file");
      return "<prepared>";
    },
  ],
  [
    "merge feature",
    (s, f) => s.merge({ workspacePath: f.wc, relativeSourcePath: "branches/feature" }),
  ],
  ["changes merged", (s, f) => s.getChanges({ workspacePath: f.wc })],
  ["diff merged", (s, f) => s.getDiff({ workspacePath: f.wc, path: "merged.md" })],
  ["merge invalid source", (s, f) => s.merge({ workspacePath: f.wc, relativeSourcePath: "../x" })],
  [
    "merge invalid range",
    (s, f) =>
      s.merge({ workspacePath: f.wc, relativeSourcePath: "branches/feature", startRevision: 5 }),
  ],
  [
    "merge start after end",
    (s, f) =>
      s.merge({
        workspacePath: f.wc,
        relativeSourcePath: "branches/feature",
        startRevision: 9,
        endRevision: 8,
      }),
  ],
  [
    "commit merged",
    (s, f) =>
      s.commit({
        workspacePath: f.wc,
        paths: ["merged.md", "."],
        message: "test: merge feature",
      }),
  ],
  ["changes after merge commit", (s, f) => s.getChanges({ workspacePath: f.wc })],
  ["branch graph", (s, f) => s.getBranchGraph({ workspacePath: f.wc })],
  [
    "file log tracked",
    (s, f) => s.getFileLog({ workspacePath: f.wc, path: "tracked.md", limit: 2 }),
  ],
  ["file log merged", (s, f) => s.getFileLog({ workspacePath: f.wc, path: "merged.md", limit: 5 })],
  ["file log unversioned", (s, f) => s.getFileLog({ workspacePath: f.wc, path: "ignored.log" })],
  ["file log unsafe", (s, f) => s.getFileLog({ workspacePath: f.wc, path: "../x" })],
  ["blame tracked", (s, f) => s.getBlame({ workspacePath: f.wc, path: "tracked.md" })],
  [
    "blame baseline",
    (s, f) => s.getBlame({ workspacePath: f.wc, path: "tracked.md", revision: 2 }),
  ],
  ["blame unsafe", (s, f) => s.getBlame({ workspacePath: f.wc, path: "../x" })],
  [
    "prepare externals",
    async (_s, f) => {
      const definitions = join(f.root, "externals.txt");
      await writeFile(definitions, `-r 2 ^/trunk ext-a\n${f.repoUrl}/branches/feature@3 ext/b`);
      await svn(f.wc, "propset", "-q", "-F", definitions, "svn:externals", ".");
      return "<prepared>";
    },
  ],
  ["externals", (s, f) => s.getExternals({ workspacePath: f.wc })],
  [
    "prepare lock",
    async (_s, f) => {
      await writeFile(join(f.wc, "tracked.md"), "line one\nlock change\n");
      return "<prepared>";
    },
  ],
  [
    "lock tracked",
    (s, f) => s.lock({ workspacePath: f.wc, paths: ["tracked.md"], message: "hold" }),
  ],
  ["changes locked", (s, f) => s.getChanges({ workspacePath: f.wc })],
  ["unlock tracked", (s, f) => s.unlock({ workspacePath: f.wc, paths: ["tracked.md"] })],
  ["changes unlocked", (s, f) => s.getChanges({ workspacePath: f.wc })],
  [
    "prepare shelf",
    async (_s, f) => {
      await writeFile(join(f.wc, "tracked.md"), "line one\nshelf change\n");
      await writeFile(join(f.wc, "shelf-new.md"), "shelf new\n");
      return "<prepared>";
    },
  ],
  [
    "shelve",
    (s, f) =>
      s.shelve({
        workspacePath: f.wc,
        paths: ["tracked.md", "shelf-new.md"],
        name: "s1",
        message: "hold",
      }),
  ],
  ["changes shelved", (s, f) => s.getChanges({ workspacePath: f.wc })],
  ["shelves", (s, f) => s.listShelves({ workspacePath: f.wc })],
  ["unshelve", (s, f) => s.unshelve({ workspacePath: f.wc, name: "s1" })],
  ["changes unshelved", (s, f) => s.getChanges({ workspacePath: f.wc })],
  ["delete shelf", (s, f) => s.deleteShelf({ workspacePath: f.wc, name: "s1" })],
  ["shelves empty", (s, f) => s.listShelves({ workspacePath: f.wc })],
  ["cleanup", (s, f) => s.cleanup({ workspacePath: f.wc })],
];

describe("rust host svn domain", () => {
  it.skipIf(!existsSync(hostBinary) || !hasSvn())(
    "matches the TS svn service on the same fixtures",
    async () => {
      const base = await realpath(await mkdtemp(join(tmpdir(), "escode-rust-host-svn-")));
      const rootA = join(base, "a");
      const rootB = join(base, "b");
      const fixtureA = await buildRepository(rootA);
      await mkdir(rootB, { recursive: true });
      await execFileAsync(svnadminBinary, ["hotcopy", fixtureA.repo, join(rootB, "repo")]);
      const fixtureB = await checkoutCopy(rootB);
      await modifyWorkingCopy(fixtureA);
      await modifyWorkingCopy(fixtureB);

      const home = join(base, "home");
      const host = startHost(home);
      try {
        expect((await nextMeaningful(host)).kind).toBe("hostReady");
        host.send({ kind: "rpcListen", bind: "127.0.0.1:0", home });
        const listening = await nextMeaningful(host);
        expect(listening.kind).toBe("rpcListening");
        const services = await connectRpc(String(listening.addr), String(listening.secret));
        const tsSvn = createSvnService();
        const rustSvn = services.svnService;

        for (const [label, step] of STEPS) {
          const expected = normalize(await settle(step, tsSvn, fixtureA), rootA);
          const actual = normalize(await settle(step, rustSvn, fixtureB), rootB);
          expect({ label, result: actual }).toEqual({ label, result: expected });
        }
      } finally {
        await host.stop();
        await rm(base, { recursive: true, force: true });
      }
    },
    180_000,
  );
});
