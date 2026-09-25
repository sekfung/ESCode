// Run with node --import tsx. 自定义 slash 命令的 TS oracle（docs/specs/rust-custom-commands.md）：
// 1) 内置协议目录与保留名资产；2) 模板展开、参数切分、shell 展开替换与上下文变量守卫；
// 3) 真实 NodeCustomCommandAdapter 在固定目录树上的发现与加载结果。--check 防漂移。
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, relative } from "node:path";
import { BUILTIN_ZCODE_SLASH_COMMAND_HELP_ENTRIES } from "../packages/shared/src/zcode-slash-command-help.ts";
import {
  expandCustomCommandTemplate,
  formatCustomCommandPrompt,
  splitCustomCommandArguments,
} from "../apps/zcode-cli/packages/contracts/src/index.ts";
import { createNodeCustomCommandAdapter } from "../apps/zcode-cli/packages/adapters/src/commands/index.ts";
import {
  APP_PROTOCOL_APP_ONLY_BUILTIN_SLASH_COMMANDS,
  APP_PROTOCOL_VISIBLE_BUILTIN_SLASH_COMMAND_NAMES,
  isReservedZCodeSlashCommandName,
} from "../apps/zcode-cli/packages/bootstrap/src/slash-command-surface.ts";
import { expandCustomCommandShellSyntax } from "../apps/zcode-cli/packages/bootstrap/src/custom-command-shell-expansion.ts";

const check = process.argv.includes("--check");
const drift = [];
async function emit(relativeTarget, value) {
  const target = new URL(`../apps/zcode-cli-rust/crates/${relativeTarget}`, import.meta.url);
  const content = `${JSON.stringify(value, null, 2)}\n`;
  if (check) {
    if ((await readFile(target, "utf8").catch(() => "")) !== content) drift.push(relativeTarget);
  } else {
    await writeFile(target, content);
  }
}

// ---- 1. 内置目录与保留名（Rust 声明动态工作流不支持，按 TS 开关关闭剔除 workflow）----
const builtins = [
  ...APP_PROTOCOL_VISIBLE_BUILTIN_SLASH_COMMAND_NAMES.flatMap((name) => {
    const command = BUILTIN_ZCODE_SLASH_COMMAND_HELP_ENTRIES.find((entry) => entry.name === name);
    return command
      ? [{ description: command.summary, inputHint: command.usage, name, source: "builtin" }]
      : [];
  }),
  ...APP_PROTOCOL_APP_ONLY_BUILTIN_SLASH_COMMANDS,
].filter((command) => command.name !== "workflow");
const reserved = [
  ...new Set(
    BUILTIN_ZCODE_SLASH_COMMAND_HELP_ENTRIES.flatMap((e) => [e.name, ...(e.aliases ?? [])]).concat([
      "compress",
      "plan",
    ]),
  ),
].filter((name) => isReservedZCodeSlashCommandName(name));
await emit("domain/src/slash_commands.json", { builtins, reserved });

// ---- 2. 模板、参数、shell 展开 ----
const metadata = (overrides = {}) => ({
  allowedTools: [],
  description: "d",
  disableNonInteractive: false,
  frontmatterKeys: [],
  name: "review",
  path: "/x/review.md",
  rootPath: "/x",
  scope: "project",
  skills: [],
  source: "zcode",
  ...overrides,
});
const templateInputs = [
  { content: "Review $ARGUMENTS now", args: " a  b " },
  { content: "First $1 second $2 third $3", args: `one "two words" 'three\\'s'` },
  { content: "No placeholders", args: "extra args" },
  { content: "No placeholders\n\n", args: "" },
  { content: "$10 and $1", args: "a b c d e f g h i j" },
  { content: "Escaped \\ and $2", args: `a\\ b c\\` },
  { content: "Body", args: "x", skills: ["pdf", "docx"], scope: "system", source: "plugin" },
  { content: "  \n trimmed body \n", args: "" },
  { content: "$ARGUMENTS$ARGUMENTS", args: "中文 参数" },
];
const templates = templateInputs.map((input) => {
  const command = {
    metadata: metadata({
      skills: input.skills ?? [],
      scope: input.scope ?? "project",
      source: input.source ?? "zcode",
    }),
    content: input.content,
    bytesRead: 0,
    sizeBytes: 0,
    truncated: false,
  };
  const expanded = expandCustomCommandTemplate({ args: input.args, command });
  const formatted = formatCustomCommandPrompt({ ...expanded, command });
  return {
    ...input,
    body: expanded.body,
    argumentCount: expanded.argumentCount,
    prompt: formatted.prompt,
  };
});
const splits = [
  "",
  "a b",
  ` "a b" c `,
  `it\\'s 'q "x"' "\\"y"`,
  "tab\tsep\nline",
  `unterminated "quote`,
  "trailing\\",
].map((input) => ({ input, args: splitCustomCommandArguments(input) }));

const shellInputs = [
  { content: "Status: !`git status`\nDone" },
  { content: "A !`one` B !`two` C" },
  { content: "```!\nls -la\n```\nafter" },
  { content: "```! echo fenced ```x !`inline`" },
  { content: "empty !`` stays" },
  { content: "!`echo ${ZCODE_PROJECT_DIR} ${ZCODE_SESSION_ID}`", sessionId: "sess_1" },
  { content: "!`echo ${ZCODE_SESSION_ID}`" },
  { content: "!`echo ${CLAUDE_SKILL_DIR}`", sessionId: "sess_1" },
  { content: "!`echo ${ZCODE_PLUGIN_ROOT}`", sessionId: "sess_1" },
  {
    content: "!`echo ${CLAUDE_PLUGIN_ROOT}`",
    sessionId: "sess_1",
    source: "plugin",
    rootPath: "/plugins/demo/commands",
  },
  { content: "!`fail`", fail: true },
  { content: "no shell here" },
];
const shells = [];
for (const input of shellInputs) {
  const runs = [];
  const executionPort = {
    run: async (request) => {
      runs.push({
        command: request.command.command,
        cwd: request.cwd,
        env: request.env?.set ?? {},
      });
      if (input.fail) {
        return {
          status: "completed",
          exitCode: 2,
          stdout: { text: "partial" },
          stderr: { text: "  boom \n" },
        };
      }
      return {
        status: "completed",
        exitCode: 0,
        stdout: { text: `[${request.command.command}]\n\n` },
        stderr: { text: "" },
      };
    },
  };
  let output;
  let error;
  try {
    output = await expandCustomCommandShellSyntax({
      command: {
        metadata: metadata({
          source: input.source ?? "zcode",
          rootPath: input.rootPath ?? "/x",
        }),
        content: input.content,
        bytesRead: 0,
        sizeBytes: 0,
        truncated: false,
      },
      content: input.content,
      executionPort,
      sessionId: input.sessionId,
      workingDirectory: "/work",
    });
  } catch (caught) {
    error = caught instanceof Error ? caught.message : String(caught);
  }
  shells.push({ ...input, runs, ...(output !== undefined ? { output } : { error }) });
}

// ---- 3. 发现与加载 ----
const tree = {
  "home/.zcode/commands/user-only.md": "---\ndescription: From user\n---\nUser body",
  "home/.zcode/commands/shared.md": "User shared wins by priority",
  "home/.agents/commands/agents-cmd.md": "# Heading line\nbody",
  "repo/.git/HEAD": "ref: refs/heads/main",
  "repo/.zcode/commands/shared.md": "Project duplicate ignored",
  "repo/.zcode/commands/Nested/Deep.MD":
    "﻿---\r\ndescription: 'Quoted desc'\r\nargument-hint: <file>\r\nallowed-tools: [Read, Bash]\r\nskills: pdf, docx\r\ndisable-noninteractive: yes\r\nmodel: m1\r\nunknown: 1\r\n  indented: skip\r\n# comment\r\nbadline\r\n---\r\nDeep body\r\n",
  "repo/.zcode/commands/bad name.md": "invalid name",
  "repo/.zcode/commands/-dash.md": "invalid leading dash",
  "repo/.zcode/commands/empty.md": "---\ndescription:\n---\n\n",
  "repo/.zcode/commands/disabled.md": "disabled body",
  "repo/.zcode/commands/list.md": "- bullet first line\n\nmore",
  "repo/.zcode/commands/notes.txt": "not markdown",
  "repo/.agents/commands/agent-proj.md": '---\ndescription: "double"\n---\nA $ARGUMENTS',
  "repo/sub/.zcode/commands/sub-cmd.md": "Sub level command",
  "repo/sub/.zcode/commands/unterminated.md": "---\ndescription: never closed\nbody",
  "repo/sub/.zcode/commands/long.md": `---\ndescription: ${"x".repeat(1100)}\n---\nbody`,
};
const root = await mkdtemp(join(tmpdir(), "zcode-custom-commands-"));
try {
  for (const [path, content] of Object.entries(tree)) {
    await mkdir(dirname(join(root, path)), { recursive: true });
    await writeFile(join(root, path), content);
  }
  const adapter = createNodeCustomCommandAdapter({
    homeDirectory: join(root, "home"),
    disabledPaths: [join(root, "repo/.zcode/commands/disabled.md")],
  });
  const workingDirectory = join(root, "repo/sub");
  const outcome = await adapter.discoverCommands({ workingDirectory });
  const rel = (path) => relative(root, path).split(/[\\/]/).join("/");
  const commands = [];
  for (const command of outcome.commands) {
    const loaded = await adapter.loadCommand({ name: command.name, workingDirectory });
    commands.push({
      ...command,
      path: rel(command.path),
      rootPath: rel(command.rootPath),
      content: loaded.content,
    });
  }
  await emit("tools/tests/fixtures/custom_commands_corpus.json", {
    templates,
    splits,
    shells,
    discovery: {
      tree,
      home: "home",
      cwd: "repo/sub",
      disabled: ["repo/.zcode/commands/disabled.md"],
      commands,
      totalDiscovered: outcome.totalDiscovered,
    },
  });
} finally {
  await rm(root, { recursive: true, force: true });
}

if (drift.length > 0)
  throw new Error(`Rust custom command assets differ from TS: ${drift.join(", ")}`);
