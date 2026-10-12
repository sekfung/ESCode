import { afterAll, describe, expect, it } from "vitest";
import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { getBuildMetadata } from "../scripts/build-metadata.mjs";
import { getTargetPlatform } from "../scripts/target-platform.mjs";

const desktopRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const scriptPath = join(desktopRoot, "scripts", "prepare-bundled-remote-assets.mjs");

const version = getBuildMetadata().appVersion;
const remotePlatform = `linux-${getTargetPlatform().arch}`;
const otherPlatform = remotePlatform === "linux-x64" ? "linux-arm64" : "linux-x64";

const mockCdnDir = join(desktopRoot, "mock-cdn");
const bundledDir = join(desktopRoot, "bundled-remote-assets");
const releaseDir = join(mockCdnDir, "releases", version);
const bundledReleaseDir = join(bundledDir, "releases", version);

/** 与脚本一致的必需条目；缺任意一项都必须让构建失败。 */
const requiredEntries = [
  `manifest-${remotePlatform}.json`,
  join("server", "zcode-server.cjs"),
  join("node", remotePlatform, "node"),
  join("node-pty", remotePlatform, "pty.node"),
  join("glm", remotePlatform, "escode.cjs"),
  join("tools", remotePlatform, "bfs", "bfs"),
  join("tools", remotePlatform, "ripgrep", "rg"),
  join("tools", remotePlatform, "ugrep", "ugrep"),
];

// 脚本按 packages/desktop 相对路径解析输入/输出，只能就地造夹具。
// 这里沿用 desktopRuntimeEnv.test.ts 的「备份挪走 + 结束还原」约定，
// 并把它放进 fork-isolated 组，避免与其它用例并发争用 mock-cdn。
function moveAside(dir: string): string | null {
  if (!existsSync(dir)) {
    return null;
  }
  const backupDir = `${dir}.test-backup`;
  rmSync(backupDir, { recursive: true, force: true });
  renameSync(dir, backupDir);
  return backupDir;
}

function restoreAside(dir: string, backupDir: string | null): void {
  rmSync(dir, { recursive: true, force: true });
  if (backupDir) {
    renameSync(backupDir, dir);
  }
}

const mockCdnBackupDir = moveAside(mockCdnDir);
const bundledBackupDir = moveAside(bundledDir);

afterAll(() => {
  restoreAside(mockCdnDir, mockCdnBackupDir);
  restoreAside(bundledDir, bundledBackupDir);
});

function stageRelease(entries: string[]): void {
  rmSync(releaseDir, { recursive: true, force: true });
  for (const entry of entries) {
    const targetPath = join(releaseDir, entry);
    mkdirSync(dirname(targetPath), { recursive: true });
    writeFileSync(targetPath, `synthetic ${entry}`);
  }
}

function runPrepareScript(): { status: number | null; stderr: string } {
  const result = spawnSync(process.execPath, [scriptPath], {
    cwd: desktopRoot,
    encoding: "utf8",
  });
  return { status: result.status, stderr: result.stderr ?? "" };
}

describe("prepare-bundled-remote-assets", () => {
  it("只暂存构建目标架构的 linux release，并保留平台无关的 server 目录", () => {
    stageRelease([
      ...requiredEntries,
      `manifest-${otherPlatform}.json`,
      join("node", otherPlatform, "node"),
      join("tools", otherPlatform, "ripgrep", "rg"),
      join("server", "nested", "lib.js"),
    ]);

    const result = runPrepareScript();

    expect(result.status).toBe(0);
    expect(existsSync(join(bundledReleaseDir, `manifest-${remotePlatform}.json`))).toBe(true);
    expect(existsSync(join(bundledReleaseDir, "node", remotePlatform, "node"))).toBe(true);
    // 平台无关目录整棵复制。
    expect(existsSync(join(bundledReleaseDir, "server", "nested", "lib.js"))).toBe(true);
    // 非目标平台不得进包，否则安装包会带上用不到的数百 MiB 资源。
    expect(existsSync(join(bundledReleaseDir, "node", otherPlatform))).toBe(false);
    expect(existsSync(join(bundledReleaseDir, `manifest-${otherPlatform}.json`))).toBe(false);
    expect(existsSync(join(bundledReleaseDir, "tools", otherPlatform))).toBe(false);
  });

  it("release 缺少必需条目时构建失败", () => {
    stageRelease(requiredEntries.filter((entry) => !entry.includes("ugrep")));

    const result = runPrepareScript();

    expect(result.status).not.toBe(0);
    expect(result.stderr).toContain("mock-cdn release incomplete");
  });

  it("mock-cdn 版本目录缺失时构建失败", () => {
    rmSync(releaseDir, { recursive: true, force: true });

    const result = runPrepareScript();

    expect(result.status).not.toBe(0);
    expect(result.stderr).toContain("missing mock-cdn release dir");
  });
});
