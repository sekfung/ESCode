import type { BashReadonlyCommandPolicy, SafeFlagValue } from "./bash-readonly-policy-types.js";

/**
 * svn 只读子命令（docs/svn/architecture.md「Bash 策略」）。只列白名单 flag：
 * `--diff-cmd`、`--diff3-cmd`、`--config-option`、`--config-dir`、`-x/--extensions` 等
 * 能执行外部程序或改写配置的参数不在白名单内，出现即退回需要审批的路径；
 * commit / update / revert / merge / switch / resolve / cleanup 等写操作不在此表。
 */
const SVN_COMMON_FLAGS: Readonly<Record<string, SafeFlagValue>> = {
  "-q": "none",
  "--quiet": "none",
  "-r": "string",
  "--revision": "string",
  "-v": "none",
  "--verbose": "none",
  "--xml": "none",
  "--depth": "string",
  "--non-interactive": "none",
  "--username": "string",
  "--no-auth-cache": "none",
};

const SVN_STATUS_FLAGS: Readonly<Record<string, SafeFlagValue>> = {
  ...SVN_COMMON_FLAGS,
  "-u": "none",
  "--show-updates": "none",
  "--no-ignore": "none",
  "--ignore-externals": "none",
  "--changelist": "string",
  "--cl": "string",
};

const SVN_INFO_FLAGS: Readonly<Record<string, SafeFlagValue>> = {
  ...SVN_COMMON_FLAGS,
  "-R": "none",
  "--recursive": "none",
  "--show-item": "string",
  "--no-newline": "none",
};

const SVN_LOG_FLAGS: Readonly<Record<string, SafeFlagValue>> = {
  ...SVN_COMMON_FLAGS,
  "-l": "number",
  "--limit": "number",
  "-g": "none",
  "--use-merge-history": "none",
  "--stop-on-copy": "none",
  "--search": "string",
  "--search-and": "string",
  "--diff": "none",
  "-c": "string",
  "--change": "string",
};

const SVN_DIFF_FLAGS: Readonly<Record<string, SafeFlagValue>> = {
  ...SVN_COMMON_FLAGS,
  "-c": "string",
  "--change": "string",
  "--summarize": "none",
  "--git": "none",
  "--internal-diff": "none",
  "--ignore-properties": "none",
  "--properties-only": "none",
  "--show-copies-as-adds": "none",
  "--notice-ancestry": "none",
  "--old": "string",
  "--new": "string",
};

const SVN_LIST_FLAGS: Readonly<Record<string, SafeFlagValue>> = {
  ...SVN_COMMON_FLAGS,
  "-R": "none",
  "--recursive": "none",
  "--search": "string",
};

const SVN_BLAME_FLAGS: Readonly<Record<string, SafeFlagValue>> = {
  ...SVN_COMMON_FLAGS,
  "-g": "none",
  "--use-merge-history": "none",
  "--force": "none",
};

const SVN_PROP_FLAGS: Readonly<Record<string, SafeFlagValue>> = {
  ...SVN_COMMON_FLAGS,
  "-R": "none",
  "--recursive": "none",
  "--show-inherited-props": "none",
  "--revprop": "none",
};

const entry = (
  name: string,
  safeFlags: Readonly<Record<string, SafeFlagValue>>,
): readonly [string, BashReadonlyCommandPolicy] => [name, { safeFlags, respectsDoubleDash: true }];

export const READONLY_MULTIWORD_POLICY_ENTRIES_SVN = [
  entry("svn status", SVN_STATUS_FLAGS),
  entry("svn st", SVN_STATUS_FLAGS),
  entry("svn stat", SVN_STATUS_FLAGS),
  entry("svn info", SVN_INFO_FLAGS),
  entry("svn log", SVN_LOG_FLAGS),
  entry("svn diff", SVN_DIFF_FLAGS),
  entry("svn di", SVN_DIFF_FLAGS),
  entry("svn cat", SVN_COMMON_FLAGS),
  entry("svn list", SVN_LIST_FLAGS),
  entry("svn ls", SVN_LIST_FLAGS),
  entry("svn blame", SVN_BLAME_FLAGS),
  entry("svn praise", SVN_BLAME_FLAGS),
  entry("svn annotate", SVN_BLAME_FLAGS),
  entry("svn proplist", SVN_PROP_FLAGS),
  entry("svn pl", SVN_PROP_FLAGS),
  entry("svn propget", SVN_PROP_FLAGS),
  entry("svn pg", SVN_PROP_FLAGS),
] as const satisfies readonly (readonly [string, BashReadonlyCommandPolicy])[];
