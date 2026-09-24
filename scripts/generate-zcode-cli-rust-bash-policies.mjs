// Run with node --import tsx. 把 TS Bash 只读策略表导出为 Rust 内嵌资产。
// 回调以函数名导出，Rust 按名实现；新增回调或改动表项时 --check 会失败，迫使 Rust 同步。
import { readFile, writeFile } from "node:fs/promises";
import {
  GIT_READONLY_SUBCOMMAND_POLICIES,
  READONLY_MULTIWORD_COMMAND_POLICIES,
} from "../apps/zcode-cli/packages/core/src/tool/handlers/bash-readonly-policy-commands.ts";
import {
  GIT_GLOBAL_DANGEROUS_FLAGS,
  GIT_GLOBAL_NO_VALUE_FLAGS,
  GIT_GLOBAL_VALUE_FLAGS,
  READONLY_ALLOW_ANY_ARG_COMMANDS,
  READONLY_ALLOW_ANY_ARG_COMMAND_PREFIXES,
  READONLY_COMMAND_POLICIES,
} from "../apps/zcode-cli/packages/core/src/tool/handlers/bash-readonly-policy-simple-commands.ts";

const KNOWN_KEYS = new Set([
  "additionalCommandIsDangerousCallback",
  "allowAnyArgs",
  "allowCompactNumericCountFlag",
  "commandOnly",
  "regex",
  "respectsDoubleDash",
  "safeFlags",
]);

function policy(value, key) {
  for (const key of Object.keys(value)) {
    if (!KNOWN_KEYS.has(key)) throw new Error(`Unknown Bash policy key: ${key}`);
  }
  const out = {};
  if (value.safeFlags) out.safeFlags = Object.fromEntries(Object.entries(value.safeFlags).sort());
  for (const key of [
    "allowAnyArgs",
    "allowCompactNumericCountFlag",
    "commandOnly",
    "respectsDoubleDash",
  ]) {
    if (value[key] !== undefined) out[key] = value[key];
  }
  if (value.regex) out.regex = { source: value.regex.source, flags: value.regex.flags };
  if (value.additionalCommandIsDangerousCallback) {
    const name = value.additionalCommandIsDangerousCallback.name;
    // 内联箭头函数没有自身名字（会继承属性名），按策略键命名，Rust 端按此键实现。
    out.callback = name === "additionalCommandIsDangerousCallback" ? `inline:${key}` : name;
  }
  return out;
}

// 保留 Map 插入序：TS 按前缀词数做稳定排序，同长度时依赖原始顺序。
const table = (map) => [...map.entries()].map(([k, v]) => [k, policy(v, k)]);

const data = {
  commands: table(READONLY_COMMAND_POLICIES),
  multiword: table(READONLY_MULTIWORD_COMMAND_POLICIES),
  gitSubcommands: table(GIT_READONLY_SUBCOMMAND_POLICIES),
  allowAnyArgCommands: [...READONLY_ALLOW_ANY_ARG_COMMANDS].sort(),
  allowAnyArgCommandPrefixes: [...READONLY_ALLOW_ANY_ARG_COMMAND_PREFIXES],
  gitGlobalNoValueFlags: [...GIT_GLOBAL_NO_VALUE_FLAGS].sort(),
  gitGlobalValueFlags: [...GIT_GLOBAL_VALUE_FLAGS].sort(),
  gitGlobalDangerousFlags: [...GIT_GLOBAL_DANGEROUS_FLAGS].sort(),
};
const content = `${JSON.stringify(data)}\n`;
const target = new URL(
  "../apps/zcode-cli-rust/crates/domain/src/bash_policies.json",
  import.meta.url,
);
if (process.argv.includes("--check")) {
  if ((await readFile(target, "utf8")) !== content) {
    throw new Error(
      "Rust Bash policies differ from TS; run node --import tsx scripts/generate-zcode-cli-rust-bash-policies.mjs",
    );
  }
} else {
  await writeFile(target, content);
}
