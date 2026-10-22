import { relative, resolve } from "node:path";
import {
  deriveSvnBranchRef,
  getSvnRelativeRepositoryPath,
  type SvnBranchRef,
  type SvnChangesResult,
  type SvnCredentials,
  type SvnDiffRequest,
  type SvnDiffResult,
  type SvnFailure,
  type SvnFileChange,
  type SvnItemStatus,
  type SvnRequest,
  type SvnResult,
} from "@escode/shared";
import {
  DEFAULT_SVN_DIFF_BYTES,
  DEFAULT_SVN_DIFF_TIMEOUT_MS,
  DEFAULT_SVN_REMOTE_READ_TIMEOUT_MS,
  SVN_LINE_STAT_CONCURRENCY,
  SVN_LINE_STAT_MAX_FILES,
  normalizeSvnPath,
} from "./config.js";
import { svnFailure } from "./repo/svnErrors.js";
import {
  countSvnDiffLines,
  parseSvnInfoXml,
  parseSvnStatusXml,
  toSvnItemStatus,
  type SvnInfoEntry,
  type SvnStatusEntry,
} from "./repo/svnParse.js";
import {
  CONFLICT_ARTIFACT_SUFFIX,
  buildUnversionedPatch,
  isDirectory,
  isSafeRelativePath,
  localTarget,
  mapWithConcurrency,
  readUnversionedFileStat,
  splitDiffByIndex,
  toWorkspaceRelative,
} from "./svnPaths.js";
import type { SvnRuntime } from "./svnRuntime.js";

export interface SvnWorkspaceInfo {
  info: SvnInfoEntry;
  branch?: SvnBranchRef;
  canSwitch: boolean;
}

type Read<T> = { ok: true } & T;

export async function readSvnInfo(
  runtime: SvnRuntime,
  cwd: string,
  targets: string[] = ["."],
  credentials?: SvnCredentials,
): Promise<Read<{ entries: SvnInfoEntry[] }> | SvnFailure> {
  const outcome = await runtime.run({
    cwd,
    args: ["info", "--xml", "--", ...targets],
    credentials,
  });
  return outcome.ok
    ? { ok: true, entries: parseSvnInfoXml(outcome.result.stdout) }
    : outcome.failure;
}

export async function readSvnStatus(
  runtime: SvnRuntime,
  workspacePath: string,
  targets: string[] = ["."],
): Promise<Read<{ entries: SvnStatusEntry[] }> | SvnFailure> {
  const outcome = await runtime.run({
    cwd: workspacePath,
    args: ["status", "--xml", "--no-ignore", "--", ...targets],
  });
  return outcome.ok
    ? { ok: true, entries: parseSvnStatusXml(outcome.result.stdout) }
    : outcome.failure;
}

export async function countSvnConflicts(
  runtime: SvnRuntime,
  workspacePath: string,
): Promise<number> {
  const status = await readSvnStatus(runtime, workspacePath);
  return status.ok
    ? status.entries.filter((entry) => toSvnItemStatus(entry) === "conflicted").length
    : 0;
}

export async function readSvnWorkspaceInfo(
  runtime: SvnRuntime,
  params: SvnRequest,
): Promise<Read<SvnWorkspaceInfo> | SvnFailure> {
  const info = await readSvnInfo(runtime, params.workspacePath);
  if (!info.ok) {
    return info;
  }
  const entry = info.entries[0];
  if (!entry) {
    return svnFailure("not-working-copy", "not a working copy");
  }
  const relativeUrlPath =
    entry.url && entry.repositoryRoot
      ? (getSvnRelativeRepositoryPath(entry.url, entry.repositoryRoot) ?? undefined)
      : undefined;
  const branch = relativeUrlPath !== undefined ? deriveSvnBranchRef(relativeUrlPath) : undefined;
  // 只有工作副本根正好是分支根时才允许 switch：workspace 是子目录时，
  // 切换的是整个工作副本；工作副本根是仓库根时没有可切换的分支。
  let canSwitch = false;
  if (branch && branch.kind !== "other" && entry.wcRoot && relativeUrlPath !== undefined) {
    const workspaceInWc = normalizeSvnPath(relative(entry.wcRoot, params.workspacePath));
    const wcRootRelativeUrl =
      workspaceInWc && workspaceInWc !== "."
        ? relativeUrlPath.slice(0, Math.max(0, relativeUrlPath.length - workspaceInWc.length - 1))
        : relativeUrlPath;
    canSwitch = wcRootRelativeUrl === branch.relativePath;
  }
  return { ok: true, info: entry, ...(branch ? { branch } : {}), canSwitch };
}

async function collectConflictDetails(
  runtime: SvnRuntime,
  workspacePath: string,
  conflictedPaths: string[],
): Promise<Map<string, SvnInfoEntry>> {
  const details = new Map<string, SvnInfoEntry>();
  if (conflictedPaths.length === 0) {
    return details;
  }
  const info = await readSvnInfo(runtime, workspacePath, conflictedPaths.map(localTarget));
  if (info.ok) {
    for (const entry of info.entries) {
      details.set(toWorkspaceRelative(workspacePath, entry.path), entry);
    }
  }
  return details;
}

function describeTreeConflict(entry: SvnInfoEntry | undefined): string | undefined {
  const tree = entry?.conflict?.treeConflict;
  if (!tree) {
    return undefined;
  }
  // svn 的 tree-conflict 属性值是稳定的英文关键字，界面层原样展示。
  return `local ${tree.reason ?? "change"}, incoming ${tree.action ?? "change"} upon ${tree.operation ?? "update"}`;
}

async function collectLineStats(
  runtime: SvnRuntime,
  workspacePath: string,
  paths: string[],
): Promise<Map<string, { additions: number; deletions: number; isBinary: boolean }>> {
  const stats = new Map<string, { additions: number; deletions: number; isBinary: boolean }>();
  // 超过上限时不统计行数，避免大工作副本打开面板时被一次超大 diff 卡住。
  if (paths.length === 0 || paths.length > SVN_LINE_STAT_MAX_FILES) {
    return stats;
  }
  const outcome = await runtime.run({
    cwd: workspacePath,
    args: ["diff", "--internal-diff", "--depth", "empty", "--", ...paths.map(localTarget)],
    timeoutMs: DEFAULT_SVN_DIFF_TIMEOUT_MS,
  });
  if (outcome.ok) {
    for (const [path, section] of splitDiffByIndex(outcome.result.stdout)) {
      stats.set(path, countSvnDiffLines(section));
    }
  }
  return stats;
}

const DIFFABLE_STATUSES: ReadonlySet<SvnItemStatus> = new Set([
  "modified",
  "added",
  "deleted",
  "replaced",
  "conflicted",
]);

export async function readSvnChanges(
  runtime: SvnRuntime,
  params: SvnRequest,
): Promise<SvnResult<SvnChangesResult>> {
  const status = await readSvnStatus(runtime, params.workspacePath);
  if (!status.ok) {
    return status;
  }
  const ignoredPaths: string[] = [];
  const visible: Array<{ entry: SvnStatusEntry; status: SvnItemStatus }> = [];
  for (const entry of status.entries) {
    if (entry.item === "ignored") {
      ignoredPaths.push(entry.path);
      continue;
    }
    const itemStatus = entry.inExternal ? null : toSvnItemStatus(entry);
    if (itemStatus) {
      visible.push({ entry, status: itemStatus });
    }
  }

  const conflictedPaths = visible
    .filter((item) => item.status === "conflicted")
    .map((item) => item.entry.path);
  const conflictDetails = await collectConflictDetails(
    runtime,
    params.workspacePath,
    conflictedPaths,
  );
  const artifactPaths = new Set(
    [...conflictDetails.values()].flatMap((entry) =>
      (entry.conflict?.artifactPaths ?? []).map((path) =>
        toWorkspaceRelative(params.workspacePath, path),
      ),
    ),
  );
  const conflictedPathSet = new Set(conflictedPaths);
  // 冲突辅助文件（.mine / .rN）是 svn 生成的临时文件，不作为未纳入版本控制的改动展示。
  const filtered = visible.filter(({ entry, status: itemStatus }) => {
    if (itemStatus !== "unversioned") {
      return true;
    }
    if (artifactPaths.has(entry.path)) {
      return false;
    }
    const base = entry.path.replace(CONFLICT_ARTIFACT_SUFFIX, "");
    return base === entry.path || !conflictedPathSet.has(base);
  });

  const lineStats = await collectLineStats(
    runtime,
    params.workspacePath,
    filtered.filter((item) => DIFFABLE_STATUSES.has(item.status)).map((item) => item.entry.path),
  );

  const changes = await mapWithConcurrency(
    filtered,
    SVN_LINE_STAT_CONCURRENCY,
    async ({ entry, status: itemStatus }): Promise<SvnFileChange> => {
      const absolutePath = resolve(params.workspacePath, entry.path);
      const detail = conflictDetails.get(entry.path);
      const description = describeTreeConflict(detail);
      const base = {
        path: entry.path,
        status: itemStatus,
        propsModified: entry.props === "modified" || entry.props === "conflicted",
        ...(entry.lock ? { lock: entry.lock } : {}),
        ...(itemStatus === "conflicted"
          ? {
              conflict: {
                kinds: detail?.conflict?.kinds ?? [entry.treeConflicted ? "tree" : "text"],
                ...(description ? { description } : {}),
              },
            }
          : {}),
      } as const;
      if (itemStatus === "unversioned") {
        const fileStat = await readUnversionedFileStat(absolutePath);
        return {
          ...base,
          isDirectory: fileStat.isDirectory,
          additions: fileStat.additions,
          deletions: 0,
          isBinary: fileStat.isBinary,
        };
      }
      const lineStat = lineStats.get(entry.path);
      return {
        ...base,
        isDirectory: await isDirectory(absolutePath),
        additions: lineStat?.additions ?? 0,
        deletions: lineStat?.deletions ?? 0,
        isBinary: lineStat?.isBinary ?? false,
      };
    },
  );

  return { ok: true, changes, ignoredPaths };
}

function toDiffResult(path: string, stdout: string, truncated: boolean): SvnDiffResult {
  return {
    path,
    patch: stdout || null,
    isBinary: countSvnDiffLines(stdout).isBinary,
    truncated,
  };
}

export async function readSvnDiff(
  runtime: SvnRuntime,
  params: SvnDiffRequest,
): Promise<SvnResult<SvnDiffResult>> {
  if (params.revision !== undefined) {
    const repoPath = params.path.startsWith("/") ? params.path : `/${params.path}`;
    const outcome = await runtime.run({
      cwd: params.workspacePath,
      args: [
        "diff",
        "--internal-diff",
        "-c",
        String(params.revision),
        "--",
        `^${repoPath}@${params.revision}`,
      ],
      timeoutMs: DEFAULT_SVN_REMOTE_READ_TIMEOUT_MS,
      maxOutputBytes: DEFAULT_SVN_DIFF_BYTES,
    });
    if (!outcome.ok && !outcome.result?.outputTruncated) {
      return outcome.failure;
    }
    const result = outcome.result!;
    return { ok: true, ...toDiffResult(params.path, result.stdout, result.outputTruncated) };
  }

  if (!isSafeRelativePath(params.path)) {
    return svnFailure("invalid-request", "path must be relative to the workspace");
  }
  const status = await readSvnStatus(runtime, params.workspacePath, [localTarget(params.path)]);
  if (!status.ok) {
    return status;
  }
  const entry = status.entries.find((item) => item.path === normalizeSvnPath(params.path));
  if (entry?.item === "unversioned") {
    const absolutePath = resolve(params.workspacePath, params.path);
    if (await isDirectory(absolutePath)) {
      return { ok: true, path: params.path, patch: null, isBinary: false, truncated: false };
    }
    return {
      ok: true,
      path: params.path,
      ...(await buildUnversionedPatch(absolutePath, params.path)),
    };
  }
  const outcome = await runtime.run({
    cwd: params.workspacePath,
    args: ["diff", "--internal-diff", "--depth", "empty", "--", localTarget(params.path)],
    timeoutMs: DEFAULT_SVN_DIFF_TIMEOUT_MS,
    maxOutputBytes: DEFAULT_SVN_DIFF_BYTES,
  });
  if (!outcome.ok && !outcome.result?.outputTruncated) {
    return outcome.failure;
  }
  const result = outcome.result!;
  return { ok: true, ...toDiffResult(params.path, result.stdout, result.outputTruncated) };
}
