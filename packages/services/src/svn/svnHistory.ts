import {
  deriveSvnBranchRef,
  getSvnRelativeRepositoryPath,
  type SvnBlameRequest,
  type SvnBlameResult,
  type SvnBranchListRequest,
  type SvnBranchListResult,
  type SvnBranchRef,
  type SvnFileLogRequest,
  type SvnFileLogResult,
  type SvnLogRequest,
  type SvnLogResult,
  type SvnRemoteStatusRequest,
  type SvnRemoteStatusResult,
  type SvnResult,
} from "@escode/shared";
import {
  DEFAULT_SVN_LOG_LIMIT,
  DEFAULT_SVN_REMOTE_READ_TIMEOUT_MS,
  MAX_SVN_BLAME_LINES,
  MAX_SVN_LOG_LIMIT,
  SVN_LOG_CACHE_MAX_ENTRIES,
} from "./config.js";
import { svnFailure } from "./repo/svnErrors.js";
import { parseSvnBlameXml, parseSvnListXml, parseSvnLogXml } from "./repo/svnParse.js";
import { childOf, childrenOf, intAttr, parseSvnXml } from "./repo/svnXml.js";
import { isSafeRelativePath, localTarget } from "./svnPaths.js";
import { readSvnInfo, readSvnWorkspaceInfo } from "./svnReads.js";
import type { SvnRuntime } from "./svnRuntime.js";

/**
 * 历史与服务器侧读取。修订一旦提交就不可变：以数字起点读取的历史页可安全缓存，
 * 从 HEAD 读取的首页不缓存（spec：docs/svn/architecture.md「状态所有者」）。
 */
export function createSvnHistoryReader(runtime: SvnRuntime) {
  const logPageCache = new Map<string, SvnLogResult>();

  return {
    async getLog(params: SvnLogRequest): Promise<SvnResult<SvnLogResult>> {
      const limit = Math.min(Math.max(params.limit ?? DEFAULT_SVN_LOG_LIMIT, 1), MAX_SVN_LOG_LIMIT);
      const workspace = await readSvnWorkspaceInfo(runtime, params);
      if (!workspace.ok) {
        return workspace;
      }
      const url = workspace.info.url;
      if (!url) {
        return svnFailure("not-working-copy", "working copy URL is unavailable");
      }
      const start = params.startRevision;
      const cacheKey =
        start !== undefined
          ? `${workspace.info.repositoryUuid ?? ""}|${url}|${start}|${limit}`
          : null;
      const cached = cacheKey ? logPageCache.get(cacheKey) : undefined;
      if (cached) {
        return { ok: true, ...cached };
      }
      // 查询 URL 而不是工作副本：工作副本默认只显示到 BASE，提交后未更新时会漏掉新修订。
      const outcome = await runtime.run({
        cwd: params.workspacePath,
        args: [
          "log",
          "--xml",
          "-v",
          "--limit",
          String(limit + 1),
          "-r",
          `${start ?? "HEAD"}:1`,
          "--",
          `${url}@${start ?? "HEAD"}`,
        ],
        credentials: params.credentials,
        timeoutMs: DEFAULT_SVN_REMOTE_READ_TIMEOUT_MS,
      });
      if (!outcome.ok) {
        return outcome.failure;
      }
      const entries = parseSvnLogXml(outcome.result.stdout);
      const page: SvnLogResult = {
        entries: entries.slice(0, limit),
        hasMore: entries.length > limit,
      };
      if (cacheKey) {
        if (logPageCache.size >= SVN_LOG_CACHE_MAX_ENTRIES) {
          const oldestKey = logPageCache.keys().next().value;
          if (oldestKey !== undefined) {
            logPageCache.delete(oldestKey);
          }
        }
        logPageCache.set(cacheKey, page);
      }
      return { ok: true, ...page };
    },

    async getFileLog(params: SvnFileLogRequest): Promise<SvnResult<SvnFileLogResult>> {
      if (!isSafeRelativePath(params.path)) {
        return svnFailure("invalid-request", "path must be relative to the workspace");
      }
      const limit = Math.min(Math.max(params.limit ?? DEFAULT_SVN_LOG_LIMIT, 1), MAX_SVN_LOG_LIMIT);
      const info = await readSvnInfo(
        runtime,
        params.workspacePath,
        [localTarget(params.path)],
        params.credentials,
      );
      if (!info.ok) {
        // `svn info` 是本地读取；路径未纳入版本控制时不同版本返回的错误码并不统一
        //（E155010 / E200009），除明确的进程级失败外统一按 invalid-request 呈现。
        return info.issue === "auth-required" ||
          info.issue === "working-copy-locked" ||
          info.issue === "svn-unavailable" ||
          info.issue === "timeout"
          ? info
          : svnFailure("invalid-request", "path is not versioned");
      }
      const entry = info.entries[0];
      const url = entry?.url;
      const repositoryRoot = entry?.repositoryRoot;
      if (!url || !repositoryRoot) {
        return svnFailure("invalid-request", "path is not versioned");
      }
      const relative = getSvnRelativeRepositoryPath(url, repositoryRoot);
      if (relative === null) {
        return svnFailure("invalid-request", "path is outside the repository");
      }
      const repositoryPath = `/${relative}`;
      const start = params.startRevision;
      const cacheKey =
        start !== undefined ? `file|${entry.repositoryUuid ?? ""}|${url}|${start}|${limit}` : null;
      const cached = cacheKey ? logPageCache.get(cacheKey) : undefined;
      if (cached) {
        return { ok: true, ...cached, repositoryPath };
      }
      const revision = start ?? "HEAD";
      const outcome = await runtime.run({
        cwd: params.workspacePath,
        args: [
          "log",
          "--xml",
          "-v",
          "--limit",
          String(limit + 1),
          "-r",
          `${revision}:1`,
          "--",
          `${url}@${revision}`,
        ],
        credentials: params.credentials,
        timeoutMs: DEFAULT_SVN_REMOTE_READ_TIMEOUT_MS,
      });
      if (!outcome.ok) {
        return outcome.failure;
      }
      const entries = parseSvnLogXml(outcome.result.stdout);
      const page: SvnLogResult = {
        entries: entries.slice(0, limit),
        hasMore: entries.length > limit,
      };
      if (cacheKey) {
        if (logPageCache.size >= SVN_LOG_CACHE_MAX_ENTRIES) {
          const oldestKey = logPageCache.keys().next().value;
          if (oldestKey !== undefined) {
            logPageCache.delete(oldestKey);
          }
        }
        logPageCache.set(cacheKey, page);
      }
      return { ok: true, ...page, repositoryPath };
    },

    async getBlame(params: SvnBlameRequest): Promise<SvnResult<SvnBlameResult>> {
      if (!isSafeRelativePath(params.path)) {
        return svnFailure("invalid-request", "path must be relative to the workspace");
      }
      const revision = params.revision !== undefined ? String(params.revision) : "BASE";
      const blameArgs = ["blame", "--xml"];
      if (params.revision !== undefined) {
        blameArgs.push("-r", revision);
      }
      blameArgs.push("--", localTarget(params.path));
      const outcome = await runtime.run({
        cwd: params.workspacePath,
        args: blameArgs,
        credentials: params.credentials,
        timeoutMs: DEFAULT_SVN_REMOTE_READ_TIMEOUT_MS,
      });
      if (!outcome.ok) {
        return outcome.failure;
      }
      const parsed = parseSvnBlameXml(outcome.result.stdout, MAX_SVN_BLAME_LINES);
      // svn 1.14 的 `blame --xml` 只给修订/作者，不包含行文本；从同一修订的
      // cat 输出取内容，保证与 blame 的 BASE（或指定修订）一致，而不是本地工作文件。
      const contentOutcome = await runtime.run({
        cwd: params.workspacePath,
        args: ["cat", "-r", revision, "--", localTarget(params.path)],
        credentials: params.credentials,
        timeoutMs: DEFAULT_SVN_REMOTE_READ_TIMEOUT_MS,
      });
      if (!contentOutcome.ok) {
        return contentOutcome.failure;
      }
      const contentLines = contentOutcome.result.stdout.split(/\r?\n/);
      if (contentLines[contentLines.length - 1] === "") {
        contentLines.pop();
      }
      return {
        ok: true,
        path: params.path,
        lines: parsed.lines.map((line, index) => ({
          ...line,
          content: contentLines[index] ?? "",
        })),
        truncated: parsed.truncated,
      };
    },

    async getRemoteStatus(
      params: SvnRemoteStatusRequest,
    ): Promise<SvnResult<SvnRemoteStatusResult>> {
      // 用 status -u 的过期条目判断，而不是比较修订号：自己刚提交的修订不算“新修订”。
      const outcome = await runtime.run({
        cwd: params.workspacePath,
        args: ["status", "--xml", "-u", "-q", "--", "."],
        credentials: params.credentials,
        timeoutMs: DEFAULT_SVN_REMOTE_READ_TIMEOUT_MS,
      });
      if (!outcome.ok) {
        return outcome.failure;
      }
      const root = parseSvnXml(outcome.result.stdout);
      let outOfDateCount = 0;
      let headRevision = 0;
      for (const target of childrenOf(root, "target")) {
        headRevision = Math.max(headRevision, intAttr(childOf(target, "against"), "revision") ?? 0);
        for (const entry of childrenOf(target, "entry")) {
          const reposItem = childOf(entry, "repos-status")?.attrs.item;
          if (reposItem && reposItem !== "none") {
            outOfDateCount += 1;
          }
        }
      }
      return { ok: true, headRevision, outOfDateCount };
    },

    async listBranches(params: SvnBranchListRequest): Promise<SvnResult<SvnBranchListResult>> {
      const workspace = await readSvnWorkspaceInfo(runtime, params);
      if (!workspace.ok) {
        return workspace;
      }
      const rootUrl = workspace.info.repositoryRoot;
      if (!rootUrl) {
        return svnFailure("not-working-copy", "repository root is unavailable");
      }
      const current = workspace.branch ? { current: workspace.branch } : {};
      const rootList = await runtime.run({
        cwd: params.workspacePath,
        args: ["list", "--xml", "--", `${rootUrl}@HEAD`],
        credentials: params.credentials,
        timeoutMs: DEFAULT_SVN_REMOTE_READ_TIMEOUT_MS,
      });
      if (!rootList.ok) {
        return rootList.failure;
      }
      const topLevel = new Set(
        parseSvnListXml(rootList.result.stdout)
          .filter((entry) => entry.kind === "dir")
          .map((entry) => entry.name),
      );
      if (!topLevel.has("trunk") || !topLevel.has("branches")) {
        return { ok: true, layout: "non-standard", branches: [], ...current };
      }
      const branchList = await runtime.run({
        cwd: params.workspacePath,
        args: ["list", "--xml", "--", `${rootUrl}/branches@HEAD`],
        credentials: params.credentials,
        timeoutMs: DEFAULT_SVN_REMOTE_READ_TIMEOUT_MS,
      });
      if (!branchList.ok) {
        return branchList.failure;
      }
      const branches: SvnBranchRef[] = [
        { kind: "trunk", name: "trunk", relativePath: "trunk" },
        ...parseSvnListXml(branchList.result.stdout)
          .filter((entry) => entry.kind === "dir" && entry.name.length > 0)
          .map((entry) => deriveSvnBranchRef(`branches/${entry.name}`))
          .sort((left, right) => left.name.localeCompare(right.name)),
      ];
      return { ok: true, layout: "standard", branches, ...current };
    },
  };
}
