// Run with node --import tsx. 以 TS isRuntimeReadOnlyBashCommand 为 oracle 导出 Bash 只读分类语料
// （docs/specs/rust-permission-modes.md「验收 2」）。语料从 TS 自己的策略表派生，覆盖安全/未知 flag、
// 位置参数、git 子命令、多词命令、写命令与复合语法；Rust 移植后逐条比对。
// xargs 在 Windows 上走平台分支（TS 读 process.platform），不纳入语料以保证产物与生成机平台无关。
import { readFile, writeFile } from "node:fs/promises";
import { isRuntimeReadOnlyBashCommand } from "../apps/zcode-cli/packages/core/src/tool/handlers/bash-semantics.ts";
import {
  GIT_READONLY_SUBCOMMAND_POLICIES,
  READONLY_MULTIWORD_COMMAND_POLICIES,
} from "../apps/zcode-cli/packages/core/src/tool/handlers/bash-readonly-policy-commands.ts";
import {
  READONLY_ALLOW_ANY_ARG_COMMANDS,
  READONLY_ALLOW_ANY_ARG_COMMAND_PREFIXES,
  READONLY_COMMAND_POLICIES,
} from "../apps/zcode-cli/packages/core/src/tool/handlers/bash-readonly-policy-simple-commands.ts";

const commands = new Set();
const add = (command) => {
  if (!/(^|[\s|;&(])xargs\b/.test(command)) commands.add(command);
};

function flagVariants(prefix, policy) {
  add(prefix);
  add(`${prefix} file.txt`);
  add(`${prefix} --zz-unknown`);
  add(`${prefix} -- -x`);
  for (const [flag, kind] of Object.entries(policy?.safeFlags ?? {})) {
    const value = kind === "none" ? "" : kind === "number" ? " 3" : kind === "char" ? " ," : " v";
    add(`${prefix} ${flag}${value}`);
    add(`${prefix} ${flag}${value} file.txt`);
    if (flag.startsWith("--") && kind !== "none") add(`${prefix} ${flag}=v`);
  }
}

for (const [name, policy] of READONLY_COMMAND_POLICIES) flagVariants(name, policy);
for (const name of READONLY_ALLOW_ANY_ARG_COMMANDS) flagVariants(name);
for (const prefix of READONLY_ALLOW_ANY_ARG_COMMAND_PREFIXES) flagVariants(String(prefix).trim());
for (const [name, policy] of READONLY_MULTIWORD_COMMAND_POLICIES) flagVariants(name, policy);
for (const [sub, policy] of GIT_READONLY_SUBCOMMAND_POLICIES) {
  flagVariants(`git ${sub}`, policy);
  add(`git --no-pager ${sub}`);
  add(`git -C other ${sub}`);
  add(`git -c core.pager=x ${sub}`);
}

const writes = [
  "rm -rf x",
  "mv a b",
  "cp a b",
  "touch x",
  "mkdir d",
  "chmod +x f",
  "tee out",
  "dd if=a of=b",
  "npm install",
  "git push",
  "git commit -m x",
  "git reset --hard",
  "git checkout -b x",
  "curl -o f http://x",
  "wget http://x",
  "sed -i s/a/b/ f",
  "sed --in-place s/a/b/ f",
  "find . -delete",
  "find . -exec rm {} ;",
  "sort -o out f",
  "python -c 'print(1)'",
  "node -e 1",
  "bash -c ls",
  "sh -c ls",
  "eval ls",
  "exec ls",
  "sudo ls",
  "env rm x",
  "nohup ls",
  "timeout 5 rm x",
  "time ls",
  "nice rm x",
];
for (const w of writes) add(w);

const readers = [
  "ls",
  "cat f",
  "git status",
  "git log --oneline -5",
  "grep -rn foo .",
  "rg foo",
  "head -5 f",
  "wc -l f",
  "pwd",
];
const templates = [
  (a) => `${a} | head -5`,
  (a) => `${a} && echo done`,
  (a) => `${a}; rm x`,
  (a) => `${a} > out.txt`,
  (a) => `${a} >> out.txt`,
  (a) => `${a} 2>/dev/null`,
  (a) => `${a} 2>&1`,
  (a) => `${a} < in.txt`,
  (a) => `FOO=1 ${a}`,
  (a) => `LANG=C ${a}`,
  (a) => `$(${a})`,
  (a) => `echo $(${a})`,
  (a) => `echo \`${a}\``,
  (a) => `(${a})`,
  (a) => `{ ${a}; }`,
  (a) => `${a} &`,
  (a) => `${a} || true`,
  (a) => `cd sub && ${a}`,
  (a) => `${a} "quoted arg"`,
  (a) => `${a} 'single'`,
  (a) => `${a} *.ts`,
  (a) => `${a} $HOME`,
  (a) => `${a} \${VAR:-x}`,
  (a) => `if true; then ${a}; fi`,
  (a) => `for f in *; do ${a}; done`,
  (a) => `${a} <<EOF\nx\nEOF`,
  (a) => `${a} | tee out`,
  (a) => `${a} | sh`,
];
for (const r of readers) for (const t of templates) add(t(r));
for (const edge of [
  "",
  " ",
  "#comment",
  "ls #x",
  "ls\nrm x",
  "ls \\\n -la",
  "'unterminated",
  "ls $(",
  "ls > /dev/null",
  "cat /etc/passwd",
  "cat \\\\server\\share\\f",
  "git status && cd .. && git log",
]) {
  add(edge);
}

const corpus = [...commands].sort();
const results = corpus
  .map((command) => (isRuntimeReadOnlyBashCommand(command) ? "1" : "0"))
  .join("");
const content = `${JSON.stringify({ commands: corpus, readOnly: results })}\n`;
const target = new URL(
  "../apps/zcode-cli-rust/crates/domain/tests/fixtures/bash_readonly_corpus.json",
  import.meta.url,
);
if (process.argv.includes("--check")) {
  if ((await readFile(target, "utf8")) !== content) {
    throw new Error(
      "Bash readonly corpus differs from TS; run node --import tsx scripts/generate-zcode-cli-rust-bash-readonly-corpus.mjs",
    );
  }
} else {
  await writeFile(target, content);
}
