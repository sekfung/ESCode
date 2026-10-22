import type {
  SvnChangedPathAction,
  SvnConflictKind,
  SvnItemStatus,
  SvnLogEntry,
} from "@escode/shared";
import { normalizeSvnPath } from "../config.js";
import { childOf, childrenOf, intAttr, parseSvnXml, pathOf, textOf } from "./svnXml.js";

export interface SvnInfoEntry {
  path: string;
  kind: "file" | "dir";
  revision?: number;
  url?: string;
  repositoryRoot?: string;
  repositoryUuid?: string;
  wcRoot?: string;
  lastChangedRevision?: number;
  conflict?: SvnInfoConflict;
}

export interface SvnInfoConflict {
  kinds: SvnConflictKind[];
  /** 文本冲突产生的辅助文件绝对路径（.mine / .rN 等）。 */
  artifactPaths: string[];
  treeConflict?: { operation?: string; action?: string; reason?: string; kind?: string };
}

export function parseSvnInfoXml(xml: string): SvnInfoEntry[] {
  const root = parseSvnXml(xml);
  return childrenOf(root, "entry").map((entry) => {
    const wcInfo = childOf(entry, "wc-info");
    const conflicts = childrenOf(entry, "conflict");
    const treeConflict = childOf(entry, "tree-conflict");
    const kinds = new Set<SvnConflictKind>();
    const artifactPaths: string[] = [];
    for (const conflict of conflicts) {
      kinds.add(conflict.attrs.type === "property" ? "property" : "text");
      for (const name of ["prev-base-file", "prev-wc-file", "cur-base-file", "prop-file"]) {
        const file = textOf(childOf(conflict, name))?.trim();
        if (file) {
          artifactPaths.push(file);
        }
      }
    }
    if (treeConflict) {
      kinds.add("tree");
    }
    return {
      path: entry.attrs.path ?? ".",
      kind: entry.attrs.kind === "file" ? "file" : "dir",
      revision: intAttr(entry, "revision"),
      url: textOf(childOf(entry, "url"))?.trim(),
      repositoryRoot: textOf(pathOf(entry, "repository", "root"))?.trim(),
      repositoryUuid: textOf(pathOf(entry, "repository", "uuid"))?.trim(),
      wcRoot: textOf(childOf(wcInfo, "wcroot-abspath"))?.trim(),
      lastChangedRevision: intAttr(childOf(entry, "commit"), "revision"),
      ...(kinds.size > 0
        ? {
            conflict: {
              kinds: [...kinds],
              artifactPaths,
              ...(treeConflict
                ? {
                    treeConflict: {
                      operation: treeConflict.attrs.operation,
                      action: treeConflict.attrs.action,
                      reason: treeConflict.attrs.reason,
                      kind: treeConflict.attrs.kind,
                    },
                  }
                : {}),
            },
          }
        : {}),
    };
  });
}

export interface SvnStatusEntry {
  path: string;
  item: string;
  props: string;
  treeConflicted: boolean;
  /** externals 定义下的条目属于其他工作副本，第一期只读展示。 */
  inExternal: boolean;
}

export function parseSvnStatusXml(xml: string): SvnStatusEntry[] {
  const root = parseSvnXml(xml);
  const entries: SvnStatusEntry[] = [];
  for (const target of childrenOf(root, "target")) {
    for (const entry of childrenOf(target, "entry")) {
      const wcStatus = childOf(entry, "wc-status");
      entries.push({
        path: normalizeSvnPath(entry.attrs.path ?? "."),
        item: wcStatus?.attrs.item ?? "normal",
        props: wcStatus?.attrs.props ?? "none",
        treeConflicted: wcStatus?.attrs["tree-conflicted"] === "true",
        inExternal: false,
      });
    }
  }
  // 状态输出会把每个 external 作为单独的 target 追加，路径以外部目录为前缀。
  const externalRoots = entries.filter((entry) => entry.item === "external").map((e) => e.path);
  for (const entry of entries) {
    entry.inExternal = externalRoots.some(
      (externalRoot) => entry.path !== externalRoot && entry.path.startsWith(`${externalRoot}/`),
    );
  }
  return entries;
}

/** 把 svn wc-status 映射到界面状态；返回 null 表示不展示（normal / ignored 等）。 */
export function toSvnItemStatus(entry: SvnStatusEntry): SvnItemStatus | null {
  if (entry.treeConflicted || entry.item === "conflicted" || entry.props === "conflicted") {
    return "conflicted";
  }
  switch (entry.item) {
    case "modified":
    case "merged":
      return "modified";
    case "added":
      return "added";
    case "deleted":
      return "deleted";
    case "replaced":
      return "replaced";
    case "unversioned":
      return "unversioned";
    case "missing":
      return "missing";
    case "obstructed":
    case "incomplete":
      return "obstructed";
    case "external":
      return "external";
    default:
      return entry.props === "modified" ? "props-modified" : null;
  }
}

export function parseSvnLogXml(xml: string): SvnLogEntry[] {
  const root = parseSvnXml(xml);
  return childrenOf(root, "logentry").map((entry) => ({
    revision: intAttr(entry, "revision") ?? 0,
    author: textOf(childOf(entry, "author"))?.trim() ?? "",
    date: textOf(childOf(entry, "date"))?.trim() ?? "",
    message: textOf(childOf(entry, "msg")) ?? "",
    changedPaths: childrenOf(childOf(entry, "paths"), "path").map((path) => {
      const action = path.attrs.action;
      const copyFromRevision = intAttr(path, "copyfrom-rev");
      return {
        action: (["A", "M", "D", "R"].includes(action ?? "")
          ? action
          : "M") as SvnChangedPathAction,
        path: path.text.trim(),
        ...(path.attrs.kind === "file" || path.attrs.kind === "dir"
          ? { kind: path.attrs.kind }
          : {}),
        ...(path.attrs["copyfrom-path"] ? { copyFromPath: path.attrs["copyfrom-path"] } : {}),
        ...(copyFromRevision !== undefined ? { copyFromRevision } : {}),
      };
    }),
  }));
}

export interface SvnListEntry {
  name: string;
  kind: "file" | "dir";
}

export function parseSvnListXml(xml: string): SvnListEntry[] {
  const root = parseSvnXml(xml);
  return childrenOf(root, "list").flatMap((list) =>
    childrenOf(list, "entry").map((entry) => ({
      name: textOf(childOf(entry, "name"))?.trim() ?? "",
      kind: entry.attrs.kind === "file" ? ("file" as const) : ("dir" as const),
    })),
  );
}

/**
 * 统计 update / switch 输出中的变更行。这些行的前四列是固定的状态字母，
 * 不受本地化影响；汇总文本（"Updated to revision N."）会被翻译，不解析。
 */
export function countSvnUpdatedPaths(stdout: string): number {
  return stdout
    .split(/\r?\n/)
    .filter((line) => /^[ADUCGER ][ADUCGE ][B ][C ] +\S/.test(line) && /\S/.test(line.slice(0, 4)))
    .length;
}

/** 提取 commit 输出最后一个数字作为新修订号；成功输出的最后一行总是包含修订号。 */
export function parseSvnCommittedRevision(stdout: string): number | null {
  const lines = stdout.split(/\r?\n/).filter((line) => line.trim().length > 0);
  const lastLine = lines[lines.length - 1];
  const match = lastLine?.match(/(\d+)\D*$/);
  return match ? Number.parseInt(match[1]!, 10) : null;
}

/**
 * 统计 diff 的增删行数。只计 hunk（@@ 之后）中的行，跳过文件头与属性差异段；
 * 二进制文件的提示固定带有 `svn:mime-type = ` 行（属性名不会被本地化）。
 */
export function countSvnDiffLines(patch: string): {
  additions: number;
  deletions: number;
  isBinary: boolean;
} {
  let additions = 0;
  let deletions = 0;
  let isBinary = false;
  let inHunk = false;
  for (const line of patch.split(/\r?\n/)) {
    if (line.startsWith("Index: ") || line.startsWith("Property changes on: ")) {
      inHunk = false;
      continue;
    }
    if (line.startsWith("@@ ")) {
      inHunk = true;
      continue;
    }
    if (!inHunk) {
      if (line.startsWith("svn:mime-type = ")) {
        isBinary = true;
      }
      continue;
    }
    if (line.startsWith("+")) {
      additions += 1;
    } else if (line.startsWith("-")) {
      deletions += 1;
    }
  }
  return { additions, deletions, isBinary };
}
