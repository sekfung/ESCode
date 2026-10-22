import { join } from "node:path";

export const DEFAULT_SVN_DISCOVERY_TIMEOUT_MS = 5_000;
export const DEFAULT_SVN_COMMAND_TIMEOUT_MS = 15_000;
export const DEFAULT_SVN_DIFF_TIMEOUT_MS = 20_000;
export const DEFAULT_SVN_REMOTE_READ_TIMEOUT_MS = 30_000;
export const DEFAULT_SVN_LONG_WRITE_TIMEOUT_MS = 10 * 60_000;
export const DEFAULT_SVN_CLEANUP_TIMEOUT_MS = 2 * 60_000;
export const DEFAULT_SVN_OUTPUT_BYTES = 4 * 1024 * 1024;
export const DEFAULT_SVN_DIFF_BYTES = 1024 * 1024;
export const DEFAULT_SVN_LOG_LIMIT = 100;
export const MAX_SVN_LOG_LIMIT = 500;
export const SVN_LOG_CACHE_MAX_ENTRIES = 2_000;
/** 统计行数时最多逐个 diff 的文件数；超过后不再统计，避免大工作副本打开面板时卡住。 */
export const SVN_LINE_STAT_MAX_FILES = 200;
export const SVN_LINE_STAT_CONCURRENCY = 4;
/** Blame 最多返回的行数；超出后截断，避免大文件一次灌入渲染器。 */
export const MAX_SVN_BLAME_LINES = 5_000;
/** 分支图谱最多展示的分支/标签数。 */
export const SVN_GRAPH_MAX_NODES = 200;
/** 并行探测 copyfrom 的分支数。 */
export const SVN_GRAPH_COPY_CONCURRENCY = 4;
/** 每个分支回看多少条日志来定位 copyfrom；超出后不展示来源。 */
export const SVN_GRAPH_COPY_LOG_LIMIT = 500;

function getWindowsTortoiseSvnCandidates(): string[] {
  return [
    join(
      process.env.ProgramW6432 ?? process.env.ProgramFiles ?? "C:\\Program Files",
      "TortoiseSVN",
      "bin",
      "svn.exe",
    ),
    join(
      process.env["ProgramFiles(x86)"] ?? "C:\\Program Files (x86)",
      "TortoiseSVN",
      "bin",
      "svn.exe",
    ),
  ];
}

/** 查找顺序：设置中手动指定 → ESCODE_SVN_BINARY → PATH → Windows TortoiseSVN 默认目录。 */
export function getSvnBinaryCandidates(configuredPath?: string): string[] {
  const candidates = [configuredPath?.trim(), process.env.ESCODE_SVN_BINARY?.trim(), "svn"];
  if (process.platform === "win32") {
    candidates.push(...getWindowsTortoiseSvnCandidates());
  }
  return [...new Set(candidates.filter((candidate): candidate is string => Boolean(candidate)))];
}

/** 会让 svn 打开交互程序的环境变量；Host 子进程里没有终端，打开即挂起。 */
const SVN_INTERACTIVE_ENV_VARS = ["SVN_EDITOR", "SVN_MERGE", "VISUAL", "EDITOR"];

const UTF8_LOCALE_PATTERN = /utf-?8/i;

/**
 * svn 子进程环境。与 Git 不同，这里不能设置 LC_ALL=C：svn 需要把本地路径从系统编码
 * 转成 UTF-8，C locale 下遇到非 ASCII 路径会直接报 E000022 “Can't convert string”。
 * Windows 上 svn 使用 UTF-16 API，不受 locale 影响。
 */
export function getSvnCommandEnv(
  baseEnv: NodeJS.ProcessEnv = process.env,
  platform: NodeJS.Platform = process.platform,
): NodeJS.ProcessEnv {
  const env: NodeJS.ProcessEnv = { ...baseEnv };
  for (const name of SVN_INTERACTIVE_ENV_VARS) {
    delete env[name];
  }
  // 设置一个必然失败的编辑器，确保 svn 即使漏掉 --non-interactive 也不会挂起等待编辑。
  env.SVN_EDITOR = platform === "win32" ? "cmd /c exit 1" : "false";
  env.TERM = "dumb";
  if (platform !== "win32") {
    const current = baseEnv.LC_ALL || baseEnv.LC_CTYPE || baseEnv.LANG || "";
    // macOS 没有 C.UTF-8，使用系统必带的 en_US.UTF-8。
    const fallback = platform === "darwin" ? "en_US.UTF-8" : "C.UTF-8";
    // LC_ALL 会覆盖所有分类，这里拆开：字符集保持 UTF-8，消息固定英文，
    // 便于解析 commit / update 输出；错误归类仍只依赖 E 错误码。
    delete env.LC_ALL;
    env.LC_CTYPE = UTF8_LOCALE_PATTERN.test(current) ? current : fallback;
    env.LC_MESSAGES = "C";
  }
  return env;
}

export function normalizeSvnPath(path: string): string {
  return path.replace(/\\/g, "/").replace(/^\.\//, "");
}

/** 只用错误码分类，不依赖本地化后的错误文本。 */
export function extractSvnErrorCodes(stderr: string): string[] {
  return [...stderr.matchAll(/\bE(\d{6})\b/g)].map((match) => `E${match[1]}`);
}

export function compareSvnVersions(left: string, right: string): number {
  const parse = (value: string) =>
    value
      .split(/[.-]/)
      .slice(0, 3)
      .map((part) => Number.parseInt(part, 10) || 0);
  const a = parse(left);
  const b = parse(right);
  for (let index = 0; index < 3; index += 1) {
    const diff = (a[index] ?? 0) - (b[index] ?? 0);
    if (diff !== 0) {
      return diff;
    }
  }
  return 0;
}
