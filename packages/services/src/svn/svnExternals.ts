import type {
  SvnExternalDefinition,
  SvnExternalsRequest,
  SvnExternalsResult,
  SvnResult,
} from "@escode/shared";
import { DEFAULT_SVN_COMMAND_TIMEOUT_MS } from "./config.js";
import { parseSvnPropertiesXml } from "./repo/svnParse.js";
import { toWorkspaceRelative } from "./svnPaths.js";
import type { SvnRuntime } from "./svnRuntime.js";

interface ParsedExternal {
  url: string;
  name: string;
  revision?: number;
  pegRevision?: number;
}

/**
 * 解析一行 svn:externals：
 *   [-r N] URL[@PEG] PATH
 * PATH 允许空格（SVN 1.8+ 的引号形式），这里只在未加引号时按剩余 token 拼接。
 */
export function parseSvnExternalLine(line: string): ParsedExternal | null {
  const trimmed = line.trim();
  if (!trimmed) {
    return null;
  }
  const tokens = trimmed.split(/\s+/);
  let index = 0;
  let revision: number | undefined;
  if (tokens[0] === "-r") {
    const value = Number(tokens[1]);
    if (!Number.isInteger(value) || value < 0) {
      return null;
    }
    revision = value;
    index = 2;
  }
  const rawUrl = tokens[index];
  if (!rawUrl || index + 1 >= tokens.length) {
    return null;
  }
  let name = tokens.slice(index + 1).join(" ");
  if (name.startsWith('"') && name.endsWith('"') && name.length >= 2) {
    name = name.slice(1, -1);
  }
  if (!name) {
    return null;
  }
  let url = rawUrl;
  let pegRevision: number | undefined;
  const pegMatch = rawUrl.match(/^(.*)@(\d+)$/);
  if (pegMatch) {
    url = pegMatch[1]!;
    pegRevision = Number(pegMatch[2]);
  }
  return {
    url,
    name,
    ...(revision !== undefined ? { revision } : {}),
    ...(pegRevision !== undefined ? { pegRevision } : {}),
  };
}

/** 只读读取工作副本本地的 svn:externals（包含未提交属性修改）。 */
export async function readSvnExternals(
  runtime: SvnRuntime,
  params: SvnExternalsRequest,
): Promise<SvnResult<SvnExternalsResult>> {
  const outcome = await runtime.run({
    cwd: params.workspacePath,
    args: ["propget", "--xml", "-R", "svn:externals", "--", "."],
    timeoutMs: DEFAULT_SVN_COMMAND_TIMEOUT_MS,
  });
  if (!outcome.ok) {
    return outcome.failure;
  }
  const externals: SvnExternalDefinition[] = [];
  for (const entry of parseSvnPropertiesXml(outcome.result.stdout, "svn:externals")) {
    const definitionPath = toWorkspaceRelative(params.workspacePath, entry.path);
    for (const line of entry.value.split(/\r?\n/)) {
      const parsed = parseSvnExternalLine(line);
      if (!parsed) {
        continue;
      }
      externals.push({ path: definitionPath, ...parsed });
    }
  }
  externals.sort((left, right) =>
    left.path === right.path
      ? left.name.localeCompare(right.name)
      : left.path.localeCompare(right.path),
  );
  return { ok: true, externals };
}
