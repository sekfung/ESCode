// Run with node --import tsx. 以 TS isRuntimeReadOnlyBashCommand 为 oracle 导出 Bash 只读分类语料
// （docs/specs/rust-permission-modes.md「验收 2」）。语料从 TS 自己的策略表派生，覆盖安全/未知 flag、
// 位置参数、git 子命令、多词命令、写命令与复合语法；Rust 移植后逐条比对。
// xargs 在 Windows 上走平台分支（TS 读 process.platform），不纳入语料以保证产物与生成机平台无关。
import { readFile, writeFile } from "node:fs/promises";
import { isRuntimeReadOnlyBashCommand } from "../apps/escode-cli/packages/core/src/tool/handlers/bash-semantics.ts";
import { analyzeBashCommand } from "../apps/escode-cli/packages/core/src/tool/handlers/bash-command-parser.ts";
import {
  GIT_READONLY_SUBCOMMAND_POLICIES,
  READONLY_MULTIWORD_COMMAND_POLICIES,
} from "../apps/escode-cli/packages/core/src/tool/handlers/bash-readonly-policy-commands.ts";
import {
  READONLY_ALLOW_ANY_ARG_COMMANDS,
  READONLY_ALLOW_ANY_ARG_COMMAND_PREFIXES,
  READONLY_COMMAND_POLICIES,
} from "../apps/escode-cli/packages/core/src/tool/handlers/bash-readonly-policy-simple-commands.ts";

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

// 语法边界（对照 MBearo/ESCode-rs 的 unbash 逐函数移植补的压力集）：引号、转义、展开、重定向与复合结构的怪癖。
const syntaxEdges = [
  "ls 'a b'",
  'ls "a b"',
  'ls "a\\"b"',
  "ls 'a'\''b'",
  "ls a\ b",
  "ls \$HOME",
  'ls "$HOME"',
  "ls '$HOME'",
  "ls $'a\nb'",
  "ls $'\x41'",
  'ls $"msg"',
  'ls "a"\'b\'c',
  "ls ''",
  'ls ""',
  "ls -- '-x'",
  "ls ~",
  "ls ~/x",
  "ls ~user",
  "ls a=b",
  "ls =x",
  "ls {a,b}",
  "ls {1..3}",
  "ls a{b,c}d",
  "ls *.{ts,js}",
  "ls ?.txt",
  "ls [ab].txt",
  "ls !(x)",
  "ls @(a|b)",
  "ls $((1+2))",
  "ls $[1+2]",
  "ls ${#x}",
  "ls ${x%.ts}",
  "ls ${x/a/b}",
  "ls ${x:0:2}",
  "ls ${!x}",
  "ls $1",
  "ls $@",
  "ls $*",
  "ls $#",
  "ls $?",
  "ls $$",
  "ls $!",
  "ls $-",
  "ls $_",
  "ls <(cat f)",
  "diff <(ls a) <(ls b)",
  "ls >(cat)",
  "ls $(echo $(pwd))",
  'ls "$(pwd)"',
  "ls `echo \`pwd\``",
  "ls 1>out",
  "ls 2>err",
  "ls &>out",
  "ls &>>out",
  "ls >|out",
  "ls <>f",
  "ls 3>&1",
  "ls >&2",
  "ls 2>&-",
  "ls <<<'x'",
  "cat <<'EOF'\nx $y\nEOF",
  "cat <<-EOF\n\tx\n\tEOF",
  'cat <<"EOF"\nx\nEOF',
  "ls | grep x | wc -l",
  "ls |& cat",
  "ls && pwd || echo no",
  "ls; ; pwd",
  "ls;",
  "ls &&",
  "! ls",
  "time ls",
  "ls & pwd",
  "(ls; pwd)",
  "( ls )",
  "{ ls; }",
  "{ls;}",
  "[[ -f x ]] && ls",
  "[ -f x ] && ls",
  "test -f x && ls",
  "(( x++ ))",
  "case x in a) ls;; esac",
  "while true; do ls; done",
  "until false; do ls; done",
  "select x in a; do ls; done",
  "f() { ls; }",
  "function f { ls; }",
  "coproc ls",
  "ls # comment with 'quote",
  "ls#nocomment",
  "ls \\n  -la \\n  dir",
  "ls\r\n",
  "ls\tdir",
  "  ls  ",
  "ls\n\npwd",
  "FOO=bar",
  "FOO=$(rm x) ls",
  "FOO='a b' ls",
  "A=1 B=2 ls",
  "export FOO=1",
  "local x=1",
  "declare -a x",
  "alias ll=ls",
  "ls 日本語.txt",
  "grep 'a|b' f",
  'grep "a\|b" f',
  "grep -e '-x' f",
  "echo \\\\",
  "echo '\'",
  "echo \"'\"",
  "echo '\"'",
  "echo $'it\'s'",
  "find . -name '*.ts' -print",
  "find . -name \*.ts",
  "git log --format='%H %s'",
  'git log --format="%H"',
  "git -c 'alias.x=!rm f' x",
  "awk '{print $1}' f",
  "sed -n '1,5p' f",
  // unbash 静默停止的边界（bash 对这些整行报语法错误、不执行）。
  "! rm x",
  "! ! ls",
  "ls | ! rm x",
  "ls ||",
  "ls |",
  "ls; ; rm x",
  "ls ;; pwd",
  ";",
  "; ls",
  "{ls;} ; rm x",
  "ls && rm x &&",
];
for (const edge of syntaxEdges) add(edge);
// 解析层 oracle：只覆盖语法形态（模板、边界、写命令），逐字段比对 Rust 子集解析器。
const syntaxCases = new Set([
  ...writes,
  ...readers.flatMap((r) => templates.map((t) => t(r))),
  ...syntaxEdges,
]);
const parseOracle = [...commands]
  .filter((command) => syntaxCases.has(command) || !/^[\w.-]+( |$)/.test(command))
  .sort()
  .map((command) => {
    const a = analyzeBashCommand(command);
    return [
      command,
      [a.hasParseErrors, a.hasUnsupportedSyntax, a.hasDynamicWords, a.hasRedirects]
        .map(Number)
        .join(""),
      a.commands.map((c) => [
        c.argv,
        c.operatorBefore ?? "",
        c.commandText,
        c.envAssignments.map((e) => `${e.name}=${e.value ?? ""}`),
        c.redirects.map((r) => `${r.fileDescriptor ?? ""}${r.operator}${r.target}`),
      ]),
    ];
  });
const corpus = [...commands].sort();
const results = corpus
  .map((command) => (isRuntimeReadOnlyBashCommand(command) ? "1" : "0"))
  .join("");
const content = `${JSON.stringify({ commands: corpus, readOnly: results, parseOracle })}\n`;
const target = new URL(
  "../apps/escode-cli-rust/crates/domain/tests/fixtures/bash_readonly_corpus.json",
  import.meta.url,
);
if (process.argv.includes("--check")) {
  if ((await readFile(target, "utf8")) !== content) {
    throw new Error(
      "Bash readonly corpus differs from TS; run node --import tsx scripts/generate-escode-cli-rust-bash-readonly-corpus.mjs",
    );
  }
} else {
  await writeFile(target, content);
}
