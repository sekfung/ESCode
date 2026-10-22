import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { resolveDetectedVcsKind, type AppSettings } from "@escode/shared";
import { createWorkspaceVcsService } from "../src/vcs/workspaceVcsService.js";

const tempDirs: string[] = [];

function makeTempDir(): string {
  const dir = mkdtempSync(join(tmpdir(), "escode-vcs-"));
  tempDirs.push(dir);
  return dir;
}

function createSettings(initial: Partial<AppSettings> = {}) {
  let settings = { ...initial } as AppSettings;
  return {
    async get() {
      return settings;
    },
    async update(patch: Partial<AppSettings>) {
      settings = { ...settings, ...patch };
    },
  };
}

afterEach(() => {
  for (const dir of tempDirs.splice(0)) {
    rmSync(dir, { recursive: true, force: true });
  }
});

describe("resolveDetectedVcsKind", () => {
  it("更近者优先，同层 Git 优先", () => {
    expect(resolveDetectedVcsKind({ gitDepth: null, svnDepth: null })).toBe("none");
    expect(resolveDetectedVcsKind({ gitDepth: 3, svnDepth: null })).toBe("git");
    expect(resolveDetectedVcsKind({ gitDepth: null, svnDepth: 3 })).toBe("svn");
    expect(resolveDetectedVcsKind({ gitDepth: 2, svnDepth: 4 })).toBe("svn");
    expect(resolveDetectedVcsKind({ gitDepth: 4, svnDepth: 2 })).toBe("git");
    expect(resolveDetectedVcsKind({ gitDepth: 3, svnDepth: 3 })).toBe("git");
  });
});

describe("WorkspaceVcsService", () => {
  it("识别只有 SVN、嵌套与同级的情况", async () => {
    const root = makeTempDir();
    mkdirSync(join(root, ".svn"));
    mkdirSync(join(root, "sub", "inner"), { recursive: true });
    const service = createWorkspaceVcsService({ settingService: createSettings() });

    expect(await service.getWorkspaceVcsInfo({ workspacePath: join(root, "sub") })).toEqual({
      kind: "svn",
      source: "detected",
      override: "auto",
      root,
    });

    // SVN 工作副本里嵌套了 git 子项目：子项目内以 Git 为准
    writeFileSync(join(root, "sub", ".git"), "gitdir: /elsewhere\n");
    expect(
      (await service.getWorkspaceVcsInfo({ workspacePath: join(root, "sub", "inner") })).kind,
    ).toBe("git");

    // 同级同时存在：Git 优先
    mkdirSync(join(root, ".git"));
    expect((await service.getWorkspaceVcsInfo({ workspacePath: root })).kind).toBe("git");
  });

  it("按身份 key 保存覆盖值，覆盖为 svn 但不是工作副本时不回退", async () => {
    const root = makeTempDir();
    mkdirSync(join(root, ".git"));
    const settings = createSettings();
    const service = createWorkspaceVcsService({ settingService: settings });
    const request = { workspacePath: root, workspaceIdentity: " ssh://host/repo " };

    const overridden = await service.setWorkspaceVcsOverride({ ...request, override: "svn" });
    expect(overridden).toEqual({ kind: "svn", source: "override", override: "svn" });
    expect((await settings.get()).workspaceVcsOverrides).toEqual({ "ssh://host/repo": "svn" });
    expect((await service.getWorkspaceVcsInfo(request)).kind).toBe("svn");
    // 不同身份不受影响
    expect((await service.getWorkspaceVcsInfo({ workspacePath: root })).kind).toBe("git");

    await service.setWorkspaceVcsOverride({ ...request, override: "auto" });
    expect((await settings.get()).workspaceVcsOverrides).toEqual({});
    expect((await service.getWorkspaceVcsInfo(request)).source).toBe("detected");
  });
});
