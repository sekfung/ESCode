import { GlobOutputSchema } from "../../../apps/zcode-cli/packages/contracts/src/tools/glob.js";
import { GrepOutputSchema } from "../../../apps/zcode-cli/packages/contracts/src/tools/grep.js";
import { WriteOutputSchema } from "../../../apps/zcode-cli/packages/contracts/src/tools/write.js";
import { EditOutputSchema } from "../../../apps/zcode-cli/packages/contracts/src/tools/edit.js";
import assert from "node:assert/strict";
import test from "node:test";
import { spawn } from "node:child_process";
import { createInterface } from "node:readline";
import { mkdtemp, rm, writeFile, readFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { once } from "node:events";
import { NodeFileSystemAdapter } from "../../../apps/zcode-cli/packages/adapters/src/fs/index.js";
import { readTextFileForModel } from "../../../apps/zcode-cli/packages/core/src/tool/handlers/read-text.js";
import { globToolEntry } from "../../../apps/zcode-cli/packages/core/src/tool/handlers/glob.js";
import { grepToolEntry } from "../../../apps/zcode-cli/packages/core/src/tool/handlers/grep.js";
import { writeToolEntry } from "../../../apps/zcode-cli/packages/core/src/tool/handlers/write.js";
import { editToolEntry } from "../../../apps/zcode-cli/packages/core/src/tool/handlers/edit.js";
import type { ToolExecutionContext } from "../../../apps/zcode-cli/packages/core/src/tool/types.js";
import {
  ReadOutputSchema,
  ReadInputJsonSchema,
} from "../../../apps/zcode-cli/packages/contracts/src/tools/read.js";
import {
  BashInputJsonSchema,
  BashOutputSchema,
} from "../../../apps/zcode-cli/packages/contracts/src/tools/bash.js";
import { TaskOutputInputJsonSchema } from "../../../apps/zcode-cli/packages/contracts/src/tools/task-output.js";
import { TaskStopInputJsonSchema } from "../../../apps/zcode-cli/packages/contracts/src/tools/task-stop.js";
import { AskUserQuestionInputJsonSchema } from "../../../apps/zcode-cli/packages/contracts/src/tools/ask-user-question.js";
import { SkillInputJsonSchema } from "../../../apps/zcode-cli/packages/contracts/src/tools/skill.js";
import {
  EnterPlanModeInputJsonSchema,
  ExitPlanModeInputJsonSchema,
} from "../../../apps/zcode-cli/packages/contracts/src/tools/plan-mode.js";
import { ReadSessionContextInputJsonSchema } from "../../../apps/zcode-cli/packages/contracts/src/tools/read-session-context.js";
import { WebFetchInputJsonSchema } from "../../../apps/zcode-cli/packages/contracts/src/tools/webfetch.js";
import { AgentInputJsonSchema } from "../../../apps/zcode-cli/packages/contracts/src/tools/agent.js";
import { SendMessageInputJsonSchema } from "../../../apps/zcode-cli/packages/contracts/src/tools/send-message.js";
import { skillToolEntry } from "../../../apps/zcode-cli/packages/core/src/tool/handlers/skill.js";
import {
  TodoReadInputJsonSchema,
  TodoWriteInputJsonSchema,
} from "../../../apps/zcode-cli/packages/contracts/src/tools/todo.js";
import {
  todoReadToolEntry,
  todoWriteToolEntry,
} from "../../../apps/zcode-cli/packages/core/src/tool/handlers/todo.js";
import { askUserQuestionToolEntry } from "../../../apps/zcode-cli/packages/core/src/tool/handlers/ask-user-question.js";

async function driver() {
  const root = await mkdtemp(join(tmpdir(), "native-tools-parity-"));
  const child = spawn(
    resolve(
      `apps/zcode-cli-rust/target/debug/examples/tool_fixture${process.platform === "win32" ? ".exe" : ""}`,
    ),
    [root],
  );
  let errors = "";
  child.stderr.on("data", (s) => {
    errors += s;
  });
  const lines = createInterface({ input: child.stdout });
  const call = async (name: string, args: unknown, session = "fixture"): Promise<any> => {
    const next = once(lines, "line");
    child.stdin.write(
      JSON.stringify(name === "definitions" ? { definitions: true } : { name, args, session }) +
        "\n",
    );
    return JSON.parse((await next)[0]);
  };
  return {
    root,
    call,
    async close() {
      const closed = once(child, "close");
      child.stdin.end();
      assert.deepEqual(await closed, [0, null], errors);
      lines.close();
      await rm(root, { recursive: true, force: true });
    },
  };
}

test("Rust tool definitions use current TS schemas and real TS file/search handlers agree on representative calls", async () => {
  const d = await driver();
  try {
    const adapter = new NodeFileSystemAdapter();
    const context = {
      workingDirectory: d.root,
      workspaceRoot: d.root,
      sessionId: "fixture",
      toolCallId: "fixture",
      traceId: "fixture",
      abortSignal: new AbortController().signal,
      fileSystemPort: adapter,
      readFileState: new Map(),
    } as unknown as ToolExecutionContext;
    const entries = {
      Write: writeToolEntry,
      Edit: editToolEntry,
      Glob: globToolEntry,
      Grep: grepToolEntry,
    };
    const schemas = {
      Agent: AgentInputJsonSchema,
      SendMessage: SendMessageInputJsonSchema,
      Skill: SkillInputJsonSchema,
      TodoRead: TodoReadInputJsonSchema,
      TodoWrite: TodoWriteInputJsonSchema,
      Read: ReadInputJsonSchema,
      Bash: BashInputJsonSchema,
      TaskOutput: TaskOutputInputJsonSchema,
      TaskStop: TaskStopInputJsonSchema,
      AskUserQuestion: AskUserQuestionInputJsonSchema,
      // plan 模式工具（docs/specs/rust-plan-mode.md）。
      EnterPlanMode: EnterPlanModeInputJsonSchema,
      ExitPlanMode: ExitPlanModeInputJsonSchema,
      WebFetch: WebFetchInputJsonSchema,
      ReadSessionContext: ReadSessionContextInputJsonSchema,
      ...Object.fromEntries(
        Object.entries(entries).map(([key, entry]) => [key, entry.inputSchema]),
      ),
    };
    for (const { function: tool } of (await d.call("definitions", {})).definitions) {
      assert.deepEqual(tool.parameters, schemas[tool.name as keyof typeof schemas]);
      if (tool.name === "TodoRead" || tool.name === "TodoWrite")
        assert.equal(
          tool.description,
          (tool.name === "TodoRead" ? todoReadToolEntry : todoWriteToolEntry).metadata.description,
        );
      if (tool.name === "AskUserQuestion")
        assert.equal(tool.description, askUserQuestionToolEntry.metadata.description);
      if (tool.name === "Skill")
        assert.equal(tool.description, skillToolEntry.metadata.description);
    }
    await writeFile(join(d.root, "a.txt"), "first\nhello 世界\nlast\n");
    for (const args of [{}, { offset: 2, limit: 1 }, { offset: 99 }]) {
      const rust = await d.call("Read", { file_path: join(d.root, "a.txt"), ...args });
      ReadOutputSchema.parse(rust.data);
      const ts = await readTextFileForModel({
        filePath: join(d.root, "a.txt"),
        fileSystemPort: adapter,
        ...args,
      });
      for (const key of ["content", "startLine", "numLines", "totalLines"] as const)
        assert.equal(rust.data[key], ts[key], `Read ${key} ${JSON.stringify(args)}`);
    }
    for (const [name, args] of [
      ["Glob", { pattern: "*.txt" }],
      ["Grep", { pattern: "hello", output_mode: "content" }],
      ["Grep", { pattern: "first\\nhello", multiline: true }],
      ["Grep", { pattern: "first|last", output_mode: "content", offset: 1, head_limit: 1 }],
    ] as const) {
      const entry = entries[name];
      const rust = await d.call(name, args);
      (name === "Glob" ? GlobOutputSchema : GrepOutputSchema).parse(rust.data);
      const ts = (await entry.handler(args, context)) as any;
      for (const key of ["filenames", "content", "numFiles", "truncated"])
        assert.deepEqual(rust.data[key], ts[key], `${name} ${key}`);
    }
    const tsPath = join(d.root, "ts.txt");
    const rustPath = join(d.root, "rust.txt");
    const normalize = (value: any) => {
      const result = { ...value };
      delete result.filePath;
      delete result.perf;
      delete result.gitDiff;
      return result;
    };
    const tsWrite = await writeToolEntry.handler(
      { file_path: tsPath, content: "alpha\nbeta\n" },
      context,
    );
    const nativeWrite = await d.call("Write", { file_path: rustPath, content: "alpha\nbeta\n" });
    WriteOutputSchema.parse(nativeWrite.data);
    assert.equal(nativeWrite.data.type, (tsWrite as any).type);
    for (const replaceAll of [false, true]) {
      const args = {
        old_string: replaceAll ? "alpha" : "beta",
        new_string: replaceAll ? "new" : "alpha",
        replace_all: replaceAll,
      };
      const ts = (await editToolEntry.handler({ file_path: tsPath, ...args }, context)) as any;
      const native = await d.call("Edit", { file_path: rustPath, ...args });
      EditOutputSchema.parse(native.data);
      assert.equal(await readFile(rustPath, "utf8"), await readFile(tsPath, "utf8"));
      const a = normalize(native.data),
        b = normalize(ts);
      delete a.structuredPatch;
      delete b.structuredPatch;
      assert.deepEqual(a, b);
    }
    // 宽松匹配（docs/specs/rust-edit-matching.md）：同一文件与参数交给 TS 与 Rust，逐项比对结果与写入内容。
    const fuzzyCases: Array<[string, Record<string, unknown>]> = [
      ["say “hi” it’s\n", { old_string: 'say "hi"', new_string: 'say "yo" it\'s' }],
      ["a\nb\nc\n", { old_string: "2: b\n3\tc", new_string: "B\nC" }],
      ["x\ty\n", { old_string: "x\\ty", new_string: "x\\ny" }],
      ["café\n", { old_string: "caf\\u00e9", new_string: "cafe" }],
      ["  keep  \n  me\nrest\n", { old_string: "keep\nme", new_string: "kept" }],
      [
        "start\n  middle line one\nend\n",
        { old_string: "start\nmiddle line onx\nend", new_string: "s" },
      ],
      ["drop\nstay\n", { old_string: "drop", new_string: "" }],
      [" a\nb\n  a\nb\n", { old_string: "a \nb", new_string: "z" }],
      ["dup dup\n", { old_string: "dup", new_string: "d" }],
      ["dup dup\n", { old_string: "du\\u0070", new_string: "d", replace_all: true }],
      ["  a\n b\n", { old_string: "a\nb", new_string: "z", replace_all: true }],
      ["nothing\n", { old_string: "missing", new_string: "x" }],
    ];
    // 每个用例的预期结果（策略名或失败），防止两侧同样失败时用例空转。
    const expected = [
      "quote_normalized",
      "line_number_prefix_stripped",
      "escape_normalized",
      "unicode_escape_normalized",
      "line_trimmed",
      "block_anchor",
      "exact",
      "failed",
      "failed",
      "unicode_escape_normalized",
      "failed",
      "failed",
    ];
    for (const [index, [initial, args]] of fuzzyCases.entries()) {
      const tsFile = join(d.root, `fuzzy-ts-${index}.txt`);
      const rustFile = join(d.root, `fuzzy-rust-${index}.txt`);
      // 经各自的 Write 建立文件，两侧都记下读取状态（编辑前必须已读）。
      await writeToolEntry.handler({ file_path: tsFile, content: initial }, context);
      await d.call("Write", { file_path: rustFile, content: initial });
      const ts = (await editToolEntry.handler({ file_path: tsFile, ...args }, context)) as any;
      const native = await d.call("Edit", { file_path: rustFile, ...args });
      const label = `fuzzy case ${index} ${JSON.stringify(args)}`;
      assert.equal(ts.result === false ? "failed" : ts.matchStrategy, expected[index], label);
      assert.equal(await readFile(rustFile, "utf8"), await readFile(tsFile, "utf8"), label);
      if (ts.result === false) {
        assert.ok(native.error, `${label}: Rust should fail like TS (${ts.message})`);
        assert.ok(String(native.error).endsWith(ts.message), `${label}: ${native.error}`);
        continue;
      }
      assert.equal(native.error, undefined, `${label}: ${native.error}`);
      EditOutputSchema.parse(native.data);
      const a = normalize(native.data),
        b = normalize(ts);
      delete a.structuredPatch;
      delete b.structuredPatch;
      assert.deepEqual(a, b, label);
    }
    if (process.platform !== "win32") {
      for (const [command, status] of [
        ["printf hello", "completed"],
        ["exit 7", "failed"],
        ["sleep 1", "timed_out"],
      ]) {
        const native = await d.call("Bash", {
          command,
          timeout: status === "timed_out" ? 10 : 1000,
        });
        BashOutputSchema.parse(native.data);
        assert.equal(native.data.status, status);
      }
    }
  } finally {
    await d.close();
  }
});
