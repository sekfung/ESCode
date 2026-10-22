import type { SvnFailure, SvnIssueCode } from "@escode/shared";
import { extractSvnErrorCodes } from "../config.js";

/**
 * svn 错误码 → 界面可处理的问题类型。只依赖 `E\d{6}` 错误码，不解析本地化文本；
 * 新增映射时同步更新 docs/svn/architecture.md 的错误归类说明。
 */
const ISSUE_BY_ERROR_CODE: Record<string, SvnIssueCode> = {
  // 工作副本被锁 / 上次操作未完成，需要 cleanup
  E155004: "working-copy-locked",
  E155037: "working-copy-locked",
  E200033: "working-copy-locked",
  // 工作副本过期，需要先 update
  E155011: "out-of-date",
  E160024: "out-of-date",
  E160028: "out-of-date",
  E170004: "out-of-date",
  E160042: "out-of-date",
  // 认证
  E170001: "auth-required",
  E215004: "auth-required",
  E170013: "auth-required",
  // 存在未解决冲突
  E155015: "unresolved-conflicts",
  // 不是工作副本
  E155007: "not-working-copy",
  E155010: "not-working-copy",
};

/** E170013 也用于无法连接服务器，只有同时带认证相关错误码时才按认证处理。 */
const AUTH_ONLY_WITH_COMPANION = new Set(["E170013"]);

export function classifySvnError(stderr: string): SvnIssueCode {
  const codes = extractSvnErrorCodes(stderr);
  for (const code of codes) {
    const issue = ISSUE_BY_ERROR_CODE[code];
    if (!issue) {
      continue;
    }
    if (AUTH_ONLY_WITH_COMPANION.has(code)) {
      const hasAuthCompanion = codes.some(
        (other) => other !== code && ISSUE_BY_ERROR_CODE[other] === "auth-required",
      );
      if (!hasAuthCompanion) {
        continue;
      }
    }
    return issue;
  }
  return "unknown";
}

/** 只保留 svn 的 `svn: Exxxxxx:` 行作为用户可读信息，去掉噪声并限制长度。 */
export function summarizeSvnStderr(stderr: string): string {
  const lines = stderr
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter((line) => line.length > 0);
  const errorLines = lines.filter((line) => /\bE\d{6}\b/.test(line));
  const summary = (errorLines.length > 0 ? errorLines : lines).join("\n");
  return summary.length > 2_000 ? `${summary.slice(0, 2_000)}…` : summary;
}

export function svnFailure(issue: SvnIssueCode, message: string): SvnFailure {
  return { ok: false, issue, message };
}
