import {
  deriveSvnBranchRef,
  type SvnBranchListRequest,
  type SvnBranchListResult,
  type SvnBranchRef,
  type SvnLogRequest,
  type SvnLogResult,
  type SvnRemoteStatusRequest,
  type SvnRemoteStatusResult,
  type SvnResult,
} from "@escode/shared";
import {
  DEFAULT_SVN_LOG_LIMIT,
  DEFAULT_SVN_REMOTE_READ_TIMEOUT_MS,
  MAX_SVN_LOG_LIMIT,
  SVN_LOG_CACHE_MAX_ENTRIES,
} from "./config.js";
import { svnFailure } from "./repo/svnErrors.js";
import { parseSvnListXml, parseSvnLogXml } from "./repo/svnParse.js";
import { childOf, childrenOf, intAttr, parseSvnXml } from "./repo/svnXml.js";
import { readSvnWorkspaceInfo } from "./svnReads.js";
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
