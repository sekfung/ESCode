import { execFile as execFileCallback } from "node:child_process";
import { stat } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { promisify } from "node:util";
import type { SvnEnvSnapshot } from "@escode/contracts";
import {
  deriveSvnBranchRef,
  getSvnRelativeRepositoryPath,
  resolveDetectedVcsKind,
  type WorkspaceVcsKind,
} from "@escode/shared";

/** 本地命令超时；svn log 访问服务器，超时后省略日志，不阻塞会话启动。 */
const SVN_COMMAND_TIMEOUT_MS = 3000;
const SVN_MAX_BUFFER_BYTES = 1024 * 1024;
const MAX_SVN_STATUS_CONTEXT_CHARS = 2000;
const MAX_RECENT_LOGS = 5;

const execFile = promisify(execFileCallback);

async function exists(path: string, kind: "any" | "dir"): Promise<boolean> {
  try {
    const info = await stat(path);
    return kind === "any" || info.isDirectory();
  } catch {
    return false;
  }
}

/**
 * 与 Host 的 workspace-vcs 判定同一规则：最近的 `.git` / `.svn` 中更近者优先，同层 Git 优先。
 * 只做目录探测，不调用 git / svn。
 */
export async function detectWorkspaceVcsKind(workingDirectory: string): Promise<WorkspaceVcsKind> {
  const chain: string[] = [];
  let current = resolve(workingDirectory);
  for (;;) {
    chain.push(current);
    const parent = dirname(current);
    if (parent === current) {
      break;
    }
    current = parent;
  }
  let gitDepth: number | null = null;
  let svnDepth: number | null = null;
  for (let index = 0; index < chain.length; index += 1) {
    const dir = chain[index]!;
    const depth = chain.length - 1 - index;
    if (gitDepth === null && (await exists(join(dir, ".git"), "any"))) {
      gitDepth = depth;
    }
    if (svnDepth === null && (await exists(join(dir, ".svn"), "dir"))) {
      svnDepth = depth;
    }
    if (gitDepth !== null && svnDepth !== null) {
      break;
    }
  }
  return resolveDetectedVcsKind({ gitDepth, svnDepth });
}

async function runSvn(args: string[], cwd: string, env: NodeJS.ProcessEnv): Promise<string | null> {
  const binary = env.ESCODE_SVN_BINARY?.trim() || "svn";
  try {
    const { stdout } = await execFile(binary, ["--non-interactive", ...args], {
      cwd,
      timeout: SVN_COMMAND_TIMEOUT_MS,
      maxBuffer: SVN_MAX_BUFFER_BYTES,
      windowsHide: true,
      // 消息固定英文；不改字符集，避免非 ASCII 路径转换失败。
      env: { ...env, LC_MESSAGES: "C", SVN_EDITOR: "false" },
    });
    return stdout;
  } catch {
    return null;
  }
}

function readXmlText(xml: string, tag: string): string | undefined {
  const match = xml.match(new RegExp(`<${tag}>([\\s\\S]*?)</${tag}>`));
  return match ? decodeXml(match[1]!.trim()) : undefined;
}

function decodeXml(value: string): string {
  return value
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">")
    .replace(/&quot;/g, '"')
    .replace(/&apos;/g, "'")
    .replace(/&amp;/g, "&");
}

function truncateStatus(output: string): string[] {
  const trimmed = output.trimEnd();
  if (!trimmed) {
    return [];
  }
  if (trimmed.length <= MAX_SVN_STATUS_CONTEXT_CHARS) {
    return trimmed.split(/\r?\n/);
  }
  return [
    ...trimmed.slice(0, MAX_SVN_STATUS_CONTEXT_CHARS).split(/\r?\n/),
    `... (truncated; run "svn status" for the full list)`,
  ];
}

function parseRecentLogs(xml: string): string[] {
  return [...xml.matchAll(/<logentry\s+revision="(\d+)">([\s\S]*?)<\/logentry>/g)]
    .slice(0, MAX_RECENT_LOGS)
    .map((match) => {
      const body = match[2]!;
      const author = readXmlText(body, "author") ?? "";
      const title = (readXmlText(body, "msg") ?? "").split(/\r?\n/).find((line) => line.trim()) ?? "";
      return `r${match[1]} ${author ? `${author}: ` : ""}${title.trim()}`;
    });
}

export async function resolveSvnSnapshot(
  workingDirectory: string,
  env: NodeJS.ProcessEnv = process.env,
): Promise<SvnEnvSnapshot> {
  const [infoXml, statusOutput] = await Promise.all([
    runSvn(["info", "--xml"], workingDirectory, env),
    runSvn(["status"], workingDirectory, env),
  ]);
  const url = infoXml ? readXmlText(infoXml, "url") : undefined;
  const repositoryRoot = infoXml ? readXmlText(infoXml, "root") : undefined;
  const revisionMatch = infoXml?.match(/<entry[^>]*\brevision="(\d+)"/);
  const relativePath =
    url && repositoryRoot ? getSvnRelativeRepositoryPath(url, repositoryRoot) : null;
  const branchRef = relativePath !== null ? deriveSvnBranchRef(relativePath) : undefined;
  const logXml = url ? await runSvn(["log", "--xml", "-l", String(MAX_RECENT_LOGS), url], workingDirectory, env) : null;

  return {
    ...(url ? { url } : {}),
    ...(revisionMatch ? { revision: Number.parseInt(revisionMatch[1]!, 10) } : {}),
    ...(branchRef
      ? { branch: branchRef.kind === "tag" ? `tag: ${branchRef.name}` : branchRef.name }
      : {}),
    statusLines: truncateStatus(statusOutput ?? ""),
    recentLogs: logXml ? parseRecentLogs(logXml) : [],
  };
}
