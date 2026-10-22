import type {
  SvnBranchGraphEdge,
  SvnBranchGraphNode,
  SvnBranchGraphRequest,
  SvnBranchGraphResult,
  SvnBranchKind,
  SvnFailure,
  SvnResult,
} from "@escode/shared";
import {
  DEFAULT_SVN_REMOTE_READ_TIMEOUT_MS,
  SVN_GRAPH_COPY_CONCURRENCY,
  SVN_GRAPH_COPY_LOG_LIMIT,
  SVN_GRAPH_MAX_NODES,
} from "./config.js";
import { svnFailure } from "./repo/svnErrors.js";
import { parseSvnListXml, parseSvnLogXml, parseSvnMergeinfoXml } from "./repo/svnParse.js";
import { mapWithConcurrency } from "./svnPaths.js";
import { readSvnWorkspaceInfo } from "./svnReads.js";
import type { SvnRuntime } from "./svnRuntime.js";

interface CandidateNode {
  relativePath: string;
  kind: SvnBranchKind;
  name: string;
}

function nodeName(relativePath: string): string {
  return relativePath.split("/").pop() ?? relativePath;
}

function encodeRepositoryPath(relativePath: string): string {
  return relativePath
    .split("/")
    .map((segment) => encodeURIComponent(segment))
    .join("/");
}

function repositoryRelativePath(path: string, rootUrl: string): string {
  const trimmedRoot = rootUrl.replace(/\/+$/, "");
  let value = path;
  if (value === trimmedRoot || value.startsWith(`${trimmedRoot}/`)) {
    value = value.slice(trimmedRoot.length);
  }
  try {
    value = decodeURIComponent(value);
  } catch {
    // 保留原始值
  }
  return value.replace(/^\/+/, "").replace(/\/+$/, "");
}

/**
 * 把仓库内路径映射回分支根：`/trunk/src` → `trunk`，`/branches/x/src` → `branches/x`。
 * 只返回已知节点，避免 mergeinfo 里的历史分支把图撑大。
 */
function branchFromRepositoryPath(path: string, known: Set<string>): string | null {
  const normalized = path.replace(/^\/+/, "").replace(/\/+$/, "");
  if (normalized === "trunk" || normalized.startsWith("trunk/")) {
    return known.has("trunk") ? "trunk" : null;
  }
  for (const prefix of ["branches/", "tags/"]) {
    if (!normalized.startsWith(prefix)) {
      continue;
    }
    const name = normalized.slice(prefix.length).split("/")[0];
    const candidate = name ? `${prefix}${name}` : "";
    return candidate && known.has(candidate) ? candidate : null;
  }
  return null;
}

async function readCopyOrigin(
  runtime: SvnRuntime,
  params: SvnBranchGraphRequest,
  rootUrl: string,
  relativePath: string,
  known: Set<string>,
): Promise<{ revision: number; from?: string } | null> {
  const url = `${rootUrl}/${encodeRepositoryPath(relativePath)}`;
  const outcome = await runtime.run({
    cwd: params.workspacePath,
    // --stop-on-copy 会从 HEAD 往回读到复制点；--limit 限制回看窗口，
    // 历史超过窗口时只保留节点、不展示来源。
    args: [
      "log",
      "--xml",
      "-v",
      "--limit",
      String(SVN_GRAPH_COPY_LOG_LIMIT),
      "--stop-on-copy",
      "--",
      `${url}@HEAD`,
    ],
    credentials: params.credentials,
    timeoutMs: DEFAULT_SVN_REMOTE_READ_TIMEOUT_MS,
  });
  if (!outcome.ok) {
    return null;
  }
  const entries = parseSvnLogXml(outcome.result.stdout);
  const oldest = entries[entries.length - 1];
  if (!oldest) {
    return null;
  }
  const targetPath = `/${relativePath}`;
  const created = oldest.changedPaths.find(
    (changed) => changed.action === "A" && changed.path.replace(/\/+$/, "") === targetPath,
  );
  if (!created) {
    return null;
  }
  const from = created.copyFromPath
    ? (branchFromRepositoryPath(created.copyFromPath, known) ?? undefined)
    : undefined;
  return { revision: oldest.revision, ...(from ? { from } : {}) };
}

function collectMergeEdges(xml: string, known: Set<string>, rootUrl: string): SvnBranchGraphEdge[] {
  const edges = new Map<string, { from: string; to: string; revisions: Set<string> }>();
  for (const entry of parseSvnMergeinfoXml(xml)) {
    // propget -R 的 target path 是仓库 URL；mergeinfo 里的来源是仓库内路径。
    const to = branchFromRepositoryPath(repositoryRelativePath(entry.path, rootUrl), known);
    if (!to) {
      continue;
    }
    for (const line of entry.mergeinfo.split(/\r?\n/)) {
      const trimmed = line.trim();
      const splitAt = trimmed.lastIndexOf(":");
      if (splitAt <= 0) {
        continue;
      }
      const from = branchFromRepositoryPath(trimmed.slice(0, splitAt), known);
      const revisions = trimmed.slice(splitAt + 1);
      if (!from || from === to || !revisions) {
        continue;
      }
      const key = `${from}\u0000${to}`;
      const edge = edges.get(key) ?? { from, to, revisions: new Set<string>() };
      for (const part of revisions.split(",")) {
        if (part.trim()) {
          edge.revisions.add(part.trim());
        }
      }
      edges.set(key, edge);
    }
  }
  return [...edges.values()]
    .sort((left, right) =>
      left.from === right.from
        ? left.to.localeCompare(right.to)
        : left.from.localeCompare(right.from),
    )
    .map((edge) => ({
      kind: "merge" as const,
      from: edge.from,
      to: edge.to,
      revisions: [...edge.revisions].join(","),
    }));
}

type DirectoryList = { ok: true; names: string[] } | { ok: false; failure: SvnFailure };

async function listDirectories(
  runtime: SvnRuntime,
  params: SvnBranchGraphRequest,
  url: string,
): Promise<DirectoryList> {
  const outcome = await runtime.run({
    cwd: params.workspacePath,
    args: ["list", "--xml", "--", `${url}@HEAD`],
    credentials: params.credentials,
    timeoutMs: DEFAULT_SVN_REMOTE_READ_TIMEOUT_MS,
  });
  if (!outcome.ok) {
    return { ok: false, failure: outcome.failure };
  }
  return {
    ok: true,
    names: parseSvnListXml(outcome.result.stdout)
      .filter((entry) => entry.kind === "dir")
      .map((entry) => entry.name),
  };
}

/**
 * 分支拓扑只读视图（spec：docs/svn/architecture.md「分支图谱」）。
 * copy 边来自每个分支的 `log --stop-on-copy`；merge 边来自仓库内 svn:mergeinfo。
 */
export async function readSvnBranchGraph(
  runtime: SvnRuntime,
  params: SvnBranchGraphRequest,
): Promise<SvnResult<SvnBranchGraphResult>> {
  const workspace = await readSvnWorkspaceInfo(runtime, params);
  if (!workspace.ok) {
    return workspace;
  }
  const rootUrl = workspace.info.repositoryRoot;
  if (!rootUrl) {
    return svnFailure("not-working-copy", "repository root is unavailable");
  }
  const topLevel = await listDirectories(runtime, params, rootUrl);
  if (!topLevel.ok) {
    return topLevel.failure;
  }
  const nonStandard = !topLevel.names.includes("trunk") || !topLevel.names.includes("branches");
  if (nonStandard) {
    const node: SvnBranchGraphNode | null = workspace.branch
      ? {
          relativePath: workspace.branch.relativePath,
          kind: workspace.branch.kind,
          name: workspace.branch.name,
        }
      : null;
    return {
      ok: true,
      layout: "non-standard",
      nodes: node ? [node] : [],
      edges: [],
      truncated: false,
    };
  }

  const candidates: CandidateNode[] = [{ relativePath: "trunk", kind: "trunk", name: "trunk" }];
  for (const [directory, kind] of [
    ["branches", "branch"],
    ["tags", "tag"],
  ] as const) {
    if (!topLevel.names.includes(directory)) {
      continue;
    }
    const names = await listDirectories(runtime, params, `${rootUrl}/${directory}`);
    if (!names.ok) {
      return names.failure;
    }
    for (const name of [...names.names].sort((left, right) => left.localeCompare(right))) {
      candidates.push({
        relativePath: `${directory}/${name}`,
        kind,
        name: nodeName(name),
      });
    }
  }
  const truncated = candidates.length > SVN_GRAPH_MAX_NODES;
  const limited = candidates.slice(0, SVN_GRAPH_MAX_NODES);
  const known = new Set(limited.map((node) => node.relativePath));

  const nodes = await mapWithConcurrency(
    limited,
    SVN_GRAPH_COPY_CONCURRENCY,
    async (candidate): Promise<SvnBranchGraphNode> => {
      if (candidate.kind === "trunk") {
        return candidate;
      }
      const origin = await readCopyOrigin(runtime, params, rootUrl, candidate.relativePath, known);
      return {
        ...candidate,
        ...(origin
          ? {
              createdRevision: origin.revision,
              ...(origin.from ? { createdFrom: origin.from } : {}),
            }
          : {}),
      };
    },
  );

  const copyEdges: SvnBranchGraphEdge[] = nodes
    .flatMap((node) =>
      node.createdFrom && node.createdRevision !== undefined
        ? [
            {
              kind: "copy" as const,
              from: node.createdFrom,
              to: node.relativePath,
              revision: node.createdRevision,
            },
          ]
        : [],
    )
    .sort((left, right) => left.to.localeCompare(right.to));

  const mergeinfoOutcome = await runtime.run({
    cwd: params.workspacePath,
    args: ["propget", "--xml", "-R", "svn:mergeinfo", "--", `${rootUrl}@HEAD`],
    credentials: params.credentials,
    timeoutMs: DEFAULT_SVN_REMOTE_READ_TIMEOUT_MS,
  });
  if (!mergeinfoOutcome.ok) {
    return mergeinfoOutcome.failure;
  }
  const mergeEdges = collectMergeEdges(mergeinfoOutcome.result.stdout, known, rootUrl);

  return {
    ok: true,
    layout: "standard",
    nodes,
    edges: [...copyEdges, ...mergeEdges],
    truncated,
  };
}
