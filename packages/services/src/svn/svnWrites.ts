import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  getWorkspaceVcsKey,
  type SvnCommitRequest,
  type SvnCommitResult,
  type SvnLockRequest,
  type SvnMergeRequest,
  type SvnMergeResult,
  type SvnRequest,
  type SvnResolveRequest,
  type SvnResult,
  type SvnRevertRequest,
  type SvnSwitchRequest,
  type SvnSwitchResult,
  type SvnUnlockRequest,
  type SvnUpdateRequest,
  type SvnUpdateResult,
} from "@escode/shared";
import {
  DEFAULT_SVN_CLEANUP_TIMEOUT_MS,
  DEFAULT_SVN_LONG_WRITE_TIMEOUT_MS,
  DEFAULT_SVN_REMOTE_READ_TIMEOUT_MS,
  normalizeSvnPath,
} from "./config.js";
import { svnFailure } from "./repo/svnErrors.js";
import {
  countSvnUpdatedPaths,
  parseSvnCommittedRevision,
  toSvnItemStatus,
} from "./repo/svnParse.js";
import { isSafeRelativePath, localTarget } from "./svnPaths.js";
import { countSvnConflicts, readSvnInfo, readSvnStatus, readSvnWorkspaceInfo } from "./svnReads.js";
import type { SvnRuntime } from "./svnRuntime.js";

async function writeMessageFile(message: string): Promise<{
  path: string;
  dispose: () => Promise<void>;
}> {
  const dir = await mkdtemp(join(tmpdir(), "escode-svn-msg-"));
  const path = join(dir, "message.txt");
  await writeFile(path, message, "utf-8");
  return { path, dispose: () => rm(dir, { recursive: true, force: true }) };
}

/** 所有写操作都经 runtime.enqueueWrite 按 workspace 身份 key 串行执行。 */
export function createSvnWriter(runtime: SvnRuntime) {
  const { logger } = runtime;

  async function commitInQueue(params: SvnCommitRequest, paths: string[]) {
    const status = await readSvnStatus(runtime, params.workspacePath, paths.map(localTarget));
    if (!status.ok) {
      return status;
    }
    const byPath = new Map(status.entries.map((entry) => [entry.path, entry]));
    const selected = paths.map((path) => ({ path, entry: byPath.get(path) }));
    if (selected.some(({ entry }) => entry && toSvnItemStatus(entry) === "conflicted")) {
      return svnFailure("unresolved-conflicts", "selected paths contain unresolved conflicts");
    }
    const toAdd = selected.filter(({ entry }) => entry?.item === "unversioned").map((s) => s.path);
    const toDelete = selected.filter(({ entry }) => entry?.item === "missing").map((s) => s.path);
    if (toAdd.length > 0) {
      const added = await runtime.run({
        cwd: params.workspacePath,
        args: ["add", "--parents", "--depth", "infinity", "--", ...toAdd.map(localTarget)],
      });
      if (!added.ok) {
        return added.failure;
      }
    }
    if (toDelete.length > 0) {
      const deleted = await runtime.run({
        cwd: params.workspacePath,
        args: ["delete", "--", ...toDelete.map(localTarget)],
      });
      if (!deleted.ok) {
        return deleted.failure;
      }
    }

    // 以 --depth empty 精确提交勾选项，避免勾选目录（例如只改了属性的 "."）时
    // 把整棵树的其他改动一起提交；新加入的目录则展开其子项一起提交。
    const targets = new Set(paths);
    for (const path of toAdd) {
      const children = await readSvnStatus(runtime, params.workspacePath, [localTarget(path)]);
      if (children.ok) {
        for (const child of children.entries) {
          if (child.item === "added") {
            targets.add(child.path);
          }
        }
      }
    }

    // 提交信息写入临时文件用 -F 传入，避免命令行转义与长度问题。
    const messageFile = await writeMessageFile(params.message);
    try {
      const outcome = await runtime.run({
        cwd: params.workspacePath,
        args: [
          "commit",
          "--depth",
          "empty",
          "-F",
          messageFile.path,
          "--encoding",
          "UTF-8",
          "--",
          ...[...targets].map(localTarget),
        ],
        credentials: params.credentials,
        timeoutMs: DEFAULT_SVN_LONG_WRITE_TIMEOUT_MS,
      });
      if (!outcome.ok) {
        return outcome.failure;
      }
      const revision = parseSvnCommittedRevision(outcome.result.stdout);
      if (revision === null) {
        return svnFailure("nothing-to-commit", "svn reported nothing to commit");
      }
      logger?.info(undefined, "svn 提交完成", {
        workspaceKey: getWorkspaceVcsKey(params),
        revision,
        fileCount: targets.size,
      });
      return { ok: true as const, revision, addedPaths: toAdd };
    } finally {
      await messageFile.dispose().catch(() => undefined);
    }
  }

  return {
    async commit(params: SvnCommitRequest): Promise<SvnResult<SvnCommitResult>> {
      const paths = [...new Set(params.paths.map(normalizeSvnPath))];
      if (paths.length === 0) {
        return svnFailure("nothing-to-commit", "no paths selected");
      }
      if (!params.message.trim()) {
        return svnFailure("invalid-request", "commit message is empty");
      }
      if (!paths.every(isSafeRelativePath)) {
        return svnFailure("invalid-request", "paths must be relative to the workspace");
      }
      return await runtime.enqueueWrite(params, () => commitInQueue(params, paths));
    },

    async update(params: SvnUpdateRequest): Promise<SvnResult<SvnUpdateResult>> {
      return await runtime.enqueueWrite(params, async () => {
        // 固定 --accept postpone：冲突留给界面处理，不依赖不同 svn 版本在非交互模式下的默认值。
        const outcome = await runtime.run({
          cwd: params.workspacePath,
          args: ["update", "--accept", "postpone", "--", "."],
          credentials: params.credentials,
          timeoutMs: DEFAULT_SVN_LONG_WRITE_TIMEOUT_MS,
        });
        if (!outcome.ok) {
          return outcome.failure;
        }
        const info = await readSvnInfo(runtime, params.workspacePath);
        const revision = info.ok ? (info.entries[0]?.revision ?? 0) : 0;
        const conflictCount = await countSvnConflicts(runtime, params.workspacePath);
        logger?.info(undefined, "svn 更新完成", {
          workspaceKey: getWorkspaceVcsKey(params),
          revision,
          conflictCount,
        });
        return {
          ok: true,
          revision,
          changedFileCount: countSvnUpdatedPaths(outcome.result.stdout),
          conflictCount,
        };
      });
    },

    async revert(params: SvnRevertRequest): Promise<SvnResult> {
      const paths = [...new Set(params.paths.map(normalizeSvnPath))];
      if (paths.length === 0 || !paths.every(isSafeRelativePath)) {
        return svnFailure("invalid-request", "paths must be relative to the workspace");
      }
      return await runtime.enqueueWrite(params, async () => {
        // 不加 --remove-added：撤销 add 后文件保留在磁盘上，与界面“还原”的确认文案一致。
        const outcome = await runtime.run({
          cwd: params.workspacePath,
          args: ["revert", "--depth", "infinity", "--", ...paths.map(localTarget)],
        });
        return outcome.ok ? { ok: true } : outcome.failure;
      });
    },

    async resolve(params: SvnResolveRequest): Promise<SvnResult> {
      if (!isSafeRelativePath(params.path)) {
        return svnFailure("invalid-request", "path must be relative to the workspace");
      }
      return await runtime.enqueueWrite(params, async () => {
        const outcome = await runtime.run({
          cwd: params.workspacePath,
          args: ["resolve", "--accept", params.accept, "--", localTarget(params.path)],
        });
        return outcome.ok ? { ok: true } : outcome.failure;
      });
    },

    async lock(params: SvnLockRequest): Promise<SvnResult> {
      const paths = [...new Set(params.paths.map(normalizeSvnPath))];
      if (paths.length === 0 || !paths.every(isSafeRelativePath)) {
        return svnFailure("invalid-request", "paths must be relative to the workspace");
      }
      return await runtime.enqueueWrite(params, async () => {
        const args = ["lock"];
        if (params.message?.trim()) {
          args.push("-m", params.message.trim());
        }
        args.push("--", ...paths.map(localTarget));
        const outcome = await runtime.run({
          cwd: params.workspacePath,
          args,
          credentials: params.credentials,
          timeoutMs: DEFAULT_SVN_REMOTE_READ_TIMEOUT_MS,
        });
        return outcome.ok ? { ok: true } : outcome.failure;
      });
    },

    async unlock(params: SvnUnlockRequest): Promise<SvnResult> {
      const paths = [...new Set(params.paths.map(normalizeSvnPath))];
      if (paths.length === 0 || !paths.every(isSafeRelativePath)) {
        return svnFailure("invalid-request", "paths must be relative to the workspace");
      }
      return await runtime.enqueueWrite(params, async () => {
        const args = ["unlock"];
        if (params.force) {
          args.push("--force");
        }
        args.push("--", ...paths.map(localTarget));
        const outcome = await runtime.run({
          cwd: params.workspacePath,
          args,
          credentials: params.credentials,
          timeoutMs: DEFAULT_SVN_REMOTE_READ_TIMEOUT_MS,
        });
        return outcome.ok ? { ok: true } : outcome.failure;
      });
    },

    async switchBranch(params: SvnSwitchRequest): Promise<SvnResult<SvnSwitchResult>> {
      const relativePath = params.relativePath.replace(/^\^?\/+/, "").replace(/\/+$/, "");
      if (!relativePath || relativePath.split("/").includes("..") || /[\r\n]/.test(relativePath)) {
        return svnFailure("invalid-request", "invalid branch path");
      }
      return await runtime.enqueueWrite(params, async () => {
        const workspace = await readSvnWorkspaceInfo(runtime, params);
        if (!workspace.ok) {
          return workspace;
        }
        const { wcRoot, repositoryRoot } = workspace.info;
        if (!wcRoot || !repositoryRoot || !workspace.canSwitch) {
          return svnFailure("invalid-request", "working copy root is not a branch root");
        }
        const targetUrl = `${repositoryRoot}/${relativePath
          .split("/")
          .map((segment) => encodeURIComponent(segment))
          .join("/")}`;
        const outcome = await runtime.run({
          cwd: wcRoot,
          args: ["switch", "--accept", "postpone", "--", `${targetUrl}@HEAD`, "."],
          credentials: params.credentials,
          timeoutMs: DEFAULT_SVN_LONG_WRITE_TIMEOUT_MS,
        });
        if (!outcome.ok) {
          return outcome.failure;
        }
        const after = await readSvnWorkspaceInfo(runtime, params);
        const conflictCount = await countSvnConflicts(runtime, params.workspacePath);
        logger?.info(undefined, "svn 切换完成", {
          workspaceKey: getWorkspaceVcsKey(params),
          branchKind: after.ok ? after.branch?.kind : undefined,
          conflictCount,
        });
        return {
          ok: true,
          revision: after.ok ? (after.info.revision ?? 0) : 0,
          ...(after.ok && after.branch ? { branch: after.branch } : {}),
          conflictCount,
        };
      });
    },

    async merge(params: SvnMergeRequest): Promise<SvnResult<SvnMergeResult>> {
      const relativeSourcePath = params.relativeSourcePath
        .replace(/^\^?\/+/, "")
        .replace(/\/+$/, "");
      if (
        !relativeSourcePath ||
        relativeSourcePath.split("/").includes("..") ||
        /[\r\n]/.test(relativeSourcePath)
      ) {
        return svnFailure("invalid-request", "invalid branch path");
      }
      const hasStart = params.startRevision !== undefined;
      const hasEnd = params.endRevision !== undefined;
      if (hasStart !== hasEnd) {
        return svnFailure("invalid-request", "revision range requires both start and end");
      }
      if (
        hasStart &&
        hasEnd &&
        (!Number.isInteger(params.startRevision) ||
          !Number.isInteger(params.endRevision) ||
          params.startRevision! < 1 ||
          params.endRevision! < 1 ||
          params.startRevision! > params.endRevision!)
      ) {
        return svnFailure("invalid-request", "invalid revision range");
      }
      return await runtime.enqueueWrite(params, async () => {
        const workspace = await readSvnWorkspaceInfo(runtime, params);
        if (!workspace.ok) {
          return workspace;
        }
        const { wcRoot, repositoryRoot } = workspace.info;
        if (!wcRoot || !repositoryRoot || !workspace.canSwitch) {
          return svnFailure("invalid-request", "working copy root is not a branch root");
        }
        const targetUrl = `${repositoryRoot}/${relativeSourcePath
          .split("/")
          .map((segment) => encodeURIComponent(segment))
          .join("/")}`;
        // UI 的起止修订是闭区间；svn 的 -r 表示两个修订之间的差异，
        // 因此区间要换成 start-1:end；单个修订用 -c 更直观（也避免 r0）。
        const range =
          hasStart && hasEnd
            ? params.startRevision === params.endRevision
              ? ["-c", String(params.startRevision)]
              : ["-r", `${Math.max(1, params.startRevision! - 1)}:${params.endRevision}`]
            : [];
        const outcome = await runtime.run({
          cwd: wcRoot,
          args: ["merge", "--accept", "postpone", ...range, "--", `${targetUrl}@HEAD`, "."],
          credentials: params.credentials,
          timeoutMs: DEFAULT_SVN_LONG_WRITE_TIMEOUT_MS,
        });
        if (!outcome.ok) {
          return outcome.failure;
        }
        const conflictCount = await countSvnConflicts(runtime, params.workspacePath);
        const mergedFileCount = countSvnUpdatedPaths(outcome.result.stdout);
        logger?.info(undefined, "svn 合并完成", {
          workspaceKey: getWorkspaceVcsKey(params),
          source: relativeSourcePath,
          range: range[1],
          mergedFileCount,
          conflictCount,
        });
        return { ok: true, mergedFileCount, conflictCount };
      });
    },

    async cleanup(params: SvnRequest): Promise<SvnResult> {
      return await runtime.enqueueWrite(params, async () => {
        const info = await readSvnInfo(runtime, params.workspacePath);
        const wcRoot = info.ok ? info.entries[0]?.wcRoot : undefined;
        const outcome = await runtime.run({
          cwd: wcRoot ?? params.workspacePath,
          args: ["cleanup", "--", "."],
          timeoutMs: DEFAULT_SVN_CLEANUP_TIMEOUT_MS,
        });
        if (outcome.ok) {
          logger?.info(undefined, "svn 清理完成", { workspaceKey: getWorkspaceVcsKey(params) });
        }
        return outcome.ok ? { ok: true } : outcome.failure;
      });
    },
  };
}
