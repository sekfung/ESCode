import { stat } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import {
  getWorkspaceVcsKey,
  resolveDetectedVcsKind,
  type WorkspaceVcsInfo,
  type WorkspaceVcsKind,
  type WorkspaceVcsOverride,
  type WorkspaceVcsRequest,
} from "@escode/shared";
import type { ISettingService } from "../setting/setting.js";
import type { IWorkspaceVcsService } from "./workspaceVcs.js";

interface VcsMarkers {
  gitRoot: string | null;
  gitDepth: number | null;
  svnRoot: string | null;
  svnDepth: number | null;
}

async function exists(path: string, kind: "any" | "dir"): Promise<boolean> {
  try {
    const info = await stat(path);
    return kind === "any" || info.isDirectory();
  } catch {
    return false;
  }
}

/**
 * 从 workspace 向上查找最近的 `.git`（目录或 worktree 文件）与 `.svn` 目录。
 * 只用异步 stat，不调用 git / svn，保证未安装二进制时也能判定类型。
 */
export async function findVcsMarkers(workspacePath: string): Promise<VcsMarkers> {
  const markers: VcsMarkers = { gitRoot: null, gitDepth: null, svnRoot: null, svnDepth: null };
  const chain: string[] = [];
  let current = resolve(workspacePath);
  for (;;) {
    chain.push(current);
    const parent = dirname(current);
    if (parent === current) {
      break;
    }
    current = parent;
  }
  // chain[0] 是 workspace 本身；深度按距文件系统根计算，越大离 workspace 越近。
  const depthOf = (index: number) => chain.length - 1 - index;
  for (let index = 0; index < chain.length; index += 1) {
    const dir = chain[index]!;
    if (markers.gitRoot === null && (await exists(join(dir, ".git"), "any"))) {
      markers.gitRoot = dir;
      markers.gitDepth = depthOf(index);
    }
    if (markers.svnRoot === null && (await exists(join(dir, ".svn"), "dir"))) {
      markers.svnRoot = dir;
      markers.svnDepth = depthOf(index);
    }
    if (markers.gitRoot !== null && markers.svnRoot !== null) {
      break;
    }
  }
  return markers;
}

function rootFor(kind: WorkspaceVcsKind, markers: VcsMarkers): string | undefined {
  if (kind === "git") {
    return markers.gitRoot ?? undefined;
  }
  if (kind === "svn") {
    return markers.svnRoot ?? undefined;
  }
  return undefined;
}

export function createWorkspaceVcsService(options: {
  settingService?: Pick<ISettingService, "get" | "update">;
}): IWorkspaceVcsService {
  async function readOverride(params: WorkspaceVcsRequest): Promise<WorkspaceVcsOverride> {
    const settings = await options.settingService?.get().catch(() => null);
    return settings?.workspaceVcsOverrides?.[getWorkspaceVcsKey(params)] ?? "auto";
  }

  async function detect(
    params: WorkspaceVcsRequest,
    override: WorkspaceVcsOverride,
  ): Promise<WorkspaceVcsInfo> {
    const markers = await findVcsMarkers(params.workspacePath);
    if (override !== "auto") {
      // 覆盖为某类型但目录不是对应仓库时不回退到另一种 VCS，root 缺省由界面提示。
      const root = rootFor(override, markers);
      return { kind: override, source: "override", override, ...(root ? { root } : {}) };
    }
    const kind = resolveDetectedVcsKind({ gitDepth: markers.gitDepth, svnDepth: markers.svnDepth });
    const root = rootFor(kind, markers);
    return { kind, source: "detected", override, ...(root ? { root } : {}) };
  }

  return {
    async getWorkspaceVcsInfo(params) {
      return await detect(params, await readOverride(params));
    },

    async setWorkspaceVcsOverride(params) {
      if (!options.settingService) {
        throw new Error("Setting service is not available.");
      }
      const key = getWorkspaceVcsKey(params);
      const settings = await options.settingService.get();
      const next = { ...settings.workspaceVcsOverrides };
      if (params.override === "auto") {
        delete next[key];
      } else {
        next[key] = params.override;
      }
      await options.settingService.update({ workspaceVcsOverrides: next });
      return await detect(params, params.override);
    },
  };
}
