import {
  type GitChangeKind,
  type SvnAvailability,
  type SvnItemStatus,
  type SvnWorkingCopyInfo,
} from "@escode/shared";
import type { ServiceLogger } from "#src/logger/serviceLogger.js";
import type { ISettingService } from "../setting/setting.js";
import type {
  CommitMessageDiffInput,
  CommitMessageFileInput,
  GitCommitMessageGenerator,
} from "../git/gitCommitMessageGenerator.js";
import { normalizeSvnPath } from "./config.js";
import type { SvnCommandProvider } from "./providers/svnCommandProvider.js";
import { svnFailure } from "./repo/svnErrors.js";
import { createSvnHistoryReader } from "./svnHistory.js";
import { readSvnChanges, readSvnDiff, readSvnWorkspaceInfo } from "./svnReads.js";
import { createSvnRuntime } from "./svnRuntime.js";
import { createSvnWriter } from "./svnWrites.js";
import type { ISvnService } from "./svn.js";

export interface SvnServiceOptions {
  commandProvider?: SvnCommandProvider;
  settingService?: Pick<ISettingService, "get" | "update">;
  commitMessageGenerator?: Pick<GitCommitMessageGenerator, "generate">;
  logger?: ServiceLogger;
}

/** 生成提交信息时最多附带的文件 diff 数；与 Git 生成器的 diff 上限保持同一量级。 */
const MAX_COMMIT_MESSAGE_DIFFS = 8;

function toCommitMessageKind(status: SvnItemStatus): GitChangeKind {
  switch (status) {
    case "added":
    case "unversioned":
      return "added";
    case "deleted":
    case "missing":
      return "deleted";
    default:
      return "modified";
  }
}

/**
 * SVN 服务组装（spec：docs/svn/architecture.md）。无状态：事实来源是工作副本本身；
 * 读操作并行，写操作按 workspace 身份 key 串行；可预期的业务失败以 `{ ok: false, issue }` 返回。
 */
export function createSvnService(options: SvnServiceOptions = {}): ISvnService {
  const runtime = createSvnRuntime(options);
  const history = createSvnHistoryReader(runtime);
  const writer = createSvnWriter(runtime);

  const service: ISvnService = {
    async getWorkingCopyInfo(params): Promise<SvnWorkingCopyInfo> {
      const binary = await runtime.resolveBinary();
      if (!binary.ok) {
        return { availability: binary.availability, isWorkingCopy: false };
      }
      const availability: SvnAvailability = {
        status: "available",
        version: binary.binary.version,
        binaryPath: binary.binary.binaryPath,
      };
      const workspace = await readSvnWorkspaceInfo(runtime, params);
      if (!workspace.ok) {
        return { availability, isWorkingCopy: false };
      }
      const { info } = workspace;
      return {
        availability,
        isWorkingCopy: true,
        ...(info.wcRoot ? { wcRoot: info.wcRoot } : {}),
        ...(info.url ? { url: info.url } : {}),
        ...(info.repositoryRoot ? { repositoryRoot: info.repositoryRoot } : {}),
        ...(info.repositoryUuid ? { repositoryUuid: info.repositoryUuid } : {}),
        ...(info.revision !== undefined ? { revision: info.revision } : {}),
        ...(workspace.branch ? { branch: workspace.branch } : {}),
        canSwitch: workspace.canSwitch,
      };
    },

    getChanges: (params) => readSvnChanges(runtime, params),
    getDiff: (params) => readSvnDiff(runtime, params),
    getLog: (params) => history.getLog(params),
    getRemoteStatus: (params) => history.getRemoteStatus(params),
    listBranches: (params) => history.listBranches(params),
    commit: (params) => writer.commit(params),
    update: (params) => writer.update(params),
    revert: (params) => writer.revert(params),
    resolve: (params) => writer.resolve(params),
    switchBranch: (params) => writer.switchBranch(params),
    merge: (params) => writer.merge(params),
    cleanup: (params) => writer.cleanup(params),

    async setBinaryPath(params) {
      if (!options.settingService) {
        throw new Error("Setting service is not available.");
      }
      const binaryPath = params.binaryPath.trim();
      await options.settingService.update({ svnBinaryPath: binaryPath || undefined });
    },

    async generateCommitMessage(params) {
      if (!options.commitMessageGenerator) {
        return svnFailure("invalid-request", "Commit message generation is not available.");
      }
      const selected = new Set(params.paths.map(normalizeSvnPath));
      const changes = await readSvnChanges(runtime, params);
      if (!changes.ok) {
        return changes;
      }
      const files = changes.changes.filter((change) => selected.has(change.path));
      if (files.length === 0) {
        return svnFailure("nothing-to-commit", "There are no changes available to commit.");
      }
      const diffs: CommitMessageDiffInput[] = [];
      for (const file of files.slice(0, MAX_COMMIT_MESSAGE_DIFFS)) {
        if (file.isDirectory || file.isBinary) {
          continue;
        }
        const diff = await readSvnDiff(runtime, { ...params, path: file.path });
        if (diff.ok && diff.patch) {
          diffs.push({ path: file.path, patch: diff.patch, summary: null });
        }
      }
      const workspace = await readSvnWorkspaceInfo(runtime, params);
      const fileInputs: CommitMessageFileInput[] = files.map((file) => ({
        kind: toCommitMessageKind(file.status),
        repoRelativePath: file.path,
        added: file.additions,
        removed: file.deletions,
      }));
      // 生成器的模型错误按 Git 同样的语义抛出，由调用方展示。
      const generated = await options.commitMessageGenerator.generate({
        workspacePath: params.workspacePath,
        ...(params.workspaceIdentity ? { workspaceIdentity: params.workspaceIdentity } : {}),
        ...(params.locale ? { locale: params.locale } : {}),
        branchName: workspace.ok ? (workspace.branch?.name ?? null) : null,
        files: fileInputs,
        diffs,
        ...(params.conversationContext ? { conversationContext: params.conversationContext } : {}),
      });
      return { ok: true, ...generated };
    },
  };

  return service;
}
