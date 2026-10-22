import { execFileSync } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { afterEach, describe, expect, it } from "vitest";
import { detectWorkspaceVcsKind, resolveSvnSnapshot } from "../src/context/svn-snapshot.js";

const tempDirs: string[] = [];

function makeTempDir(): string {
  const dir = mkdtempSync(join(tmpdir(), "escode-svn-snapshot-"));
  tempDirs.push(dir);
  return dir;
}

afterEach(() => {
  for (const dir of tempDirs.splice(0)) {
    rmSync(dir, { recursive: true, force: true });
  }
});

function findBinary(name: string): string | null {
  for (const candidate of [process.env[`ESCODE_${name.toUpperCase()}_BINARY`], name]) {
    if (!candidate) continue;
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
if (!svnBinary || !svnadminBinary) {
  console.warn("[svn-snapshot.test] 跳过真实 svn 快照测试：未找到 svn 或 svnadmin");
}

describe("detectWorkspaceVcsKind", () => {
  it("与 Host 判定一致：更近者优先，同层 Git 优先", async () => {
    const root = makeTempDir();
    mkdirSync(join(root, ".svn"));
    mkdirSync(join(root, "sub", "inner"), { recursive: true });
    expect(await detectWorkspaceVcsKind(join(root, "sub"))).toBe("svn");
    writeFileSync(join(root, "sub", ".git"), "gitdir: x\n");
    expect(await detectWorkspaceVcsKind(join(root, "sub", "inner"))).toBe("git");
    mkdirSync(join(root, ".git"));
    expect(await detectWorkspaceVcsKind(root)).toBe("git");
  });
});

describe.skipIf(!svnBinary || !svnadminBinary)("resolveSvnSnapshot（真实 svn）", () => {
  it("收集 URL、修订号、分支、状态与最近日志", async () => {
    const root = makeTempDir();
    execFileSync(svnadminBinary!, ["create", join(root, "repo")]);
    const repoUrl = pathToFileURL(join(root, "repo")).href;
    execFileSync(svnBinary!, ["mkdir", "-q", "-m", "layout", `${repoUrl}/trunk`]);
    execFileSync(svnBinary!, ["checkout", "-q", `${repoUrl}/trunk`, join(root, "wc")]);
    const wc = join(root, "wc");
    writeFileSync(join(wc, "a.txt"), "a\n");
    execFileSync(svnBinary!, ["add", "-q", "a.txt"], { cwd: wc });
    execFileSync(svnBinary!, ["commit", "-q", "-m", "feat: first\n\nbody"], { cwd: wc });
    execFileSync(svnBinary!, ["update", "-q"], { cwd: wc });
    writeFileSync(join(wc, "a.txt"), "changed\n");

    const snapshot = await resolveSvnSnapshot(wc, {
      ...process.env,
      ESCODE_SVN_BINARY: svnBinary!,
    });
    expect(snapshot).toMatchObject({
      url: `${repoUrl}/trunk`,
      revision: 2,
      branch: "trunk",
    });
    expect(snapshot.statusLines.join("\n")).toContain("a.txt");
    expect(snapshot.recentLogs[0]).toMatch(/^r2 .*feat: first$/);
  });
});
