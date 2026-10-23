import assert from "node:assert/strict";
import test from "node:test";
import { spawn } from "node:child_process";
import { createInterface } from "node:readline";
import { mkdir, mkdtemp, rm, symlink, writeFile, readFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { basename, join, relative, resolve, sep } from "node:path";
import { once } from "node:events";
import { canonical, nodeOracle } from "./escode-cli-rust-golden.js";

// Phase 3：TS 工具实现/handler/schema 只用于录制 golden；正常跑只执行 Rust 并比对冻结观察。
const contractsDir = "../../../apps/escode-cli/packages/contracts/src/tools";
const handlersDir = "../../../apps/escode-cli/packages/core/src/tool/handlers";
const fsAdapterModule = "../../../apps/escode-cli/packages/adapters/src/fs/index.js";
const readTextModule = `${handlersDir}/read-text.js`;
const pathPolicyModule = "../../../apps/escode-cli/packages/core/src/tool/path-policy.js";
const globHandlerModule = `${handlersDir}/glob.js`;
const grepHandlerModule = `${handlersDir}/grep.js`;
const writeHandlerModule = `${handlersDir}/write.js`;
const editHandlerModule = `${handlersDir}/edit.js`;
const skillHandlerModule = `${handlersDir}/skill.js`;
const todoHandlerModule = `${handlersDir}/todo.js`;
const askHandlerModule = `${handlersDir}/ask-user-question.js`;

const portable = (value: string) => value.split(sep).join("/");
/** 把临时工作区根与路径分隔符归一化，冻结观察可跨机器回放。 */
function scrub<T>(value: T, root: string): T {
  if (typeof value === "string") return value.split(root).join("<ROOT>").replaceAll("\\", "/") as T;
  if (Array.isArray(value)) return value.map((item) => scrub(item, root)) as T;
  if (value && typeof value === "object") {
    return Object.fromEntries(
      Object.entries(value).map(([key, item]) => [key, scrub(item, root)]),
    ) as T;
  }
  return value;
}
function normalizeEditOutput(value: any) {
  const result = { ...value };
  delete result.filePath;
  delete result.perf;
  delete result.gitDiff;
  delete result.structuredPatch;
  return result;
}

async function driver() {
  const root = await mkdtemp(join(tmpdir(), "native-tools-parity-"));
  const child = spawn(
    resolve(
      `apps/escode-cli-rust/target/debug/examples/tool_fixture${process.platform === "win32" ? ".exe" : ""}`,
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

test("Node and Rust echo the requested lexical tool path (dots, separators, links)", async (t) => {
  const d = await driver();
  try {
    await mkdir(join(d.root, "sub", "deep"), { recursive: true });
    await writeFile(join(d.root, "sub", "a.txt"), "hello\n");
    await writeFile(join(d.root, "up.txt"), "up\n");
    // TS `resolveWorkspacePath` 只做词法归一：折叠 `.`/`..`、统一分隔符，不解析符号链接与 8.3 短名。
    // 这些路径会原样进入模型可见的 `filePath`、App 展示与错误文案，两侧必须逐字一致。
    const readCases = [
      `${d.root}${sep}sub${sep}..${sep}sub${sep}a.txt`,
      `${d.root}/sub/deep/.././a.txt`,
      `sub${sep}..${sep}sub${sep}a.txt`,
      "./sub/a.txt",
      `sub${sep}..${sep}up.txt`,
      // 上溯到工作区外再折回（两侧都必须折叠 `..` 而不是解析符号链接）。
      `${d.root}${sep}..${sep}${basename(d.root)}${sep}sub${sep}a.txt`,
    ];
    const mirror = join(d.root, "mirror");
    const linked = join(mirror, "a.txt");
    const linkedWritten = join(mirror, "linked.txt");
    const relativeWrite = `sub${sep}..${sep}sub${sep}written.txt`;
    const tsWriteInput = `sub${sep}..${sep}sub${sep}ts-written.txt`;
    const oracle = await nodeOracle("lexical", async () => {
      const { NodeFileSystemAdapter } = await import(fsAdapterModule);
      const { readTextFileForModel } = await import(readTextModule);
      const { resolveWorkspacePath } = await import(pathPolicyModule);
      const { writeToolEntry } = await import(writeHandlerModule);
      const adapter = new NodeFileSystemAdapter();
      const expected = (inputPath: string, operation: "read" | "write") =>
        resolveWorkspacePath({
          inputPath,
          workingDirectory: d.root,
          workspaceRoot: d.root,
          operation,
        });
      const rel = (path: string) => portable(relative(d.root, path));
      const context = {
        workingDirectory: d.root,
        workspaceRoot: d.root,
        sessionId: "fixture",
        toolCallId: "fixture",
        traceId: "fixture",
        abortSignal: new AbortController().signal,
        fileSystemPort: adapter,
        readFileState: new Map(),
      };
      const reads: { filePath: string; content: string }[] = [];
      for (const inputPath of readCases) {
        const ts = await readTextFileForModel({
          filePath: expected(inputPath, "read"),
          fileSystemPort: adapter,
        });
        reads.push({ filePath: rel(ts.filePath), content: ts.content });
      }
      const tsWrite = (await writeToolEntry.handler(
        { file_path: expected(tsWriteInput, "write"), content: "written\n" },
        context as any,
      )) as any;
      return {
        reads,
        mirrorPath: rel(expected(linked, "read")),
        linkedWritePath: rel(expected(linkedWritten, "write")),
        write: {
          filePath: rel(tsWrite.filePath),
          type: tsWrite.type,
          content: tsWrite.content,
          additions: tsWrite.additions,
          deletions: tsWrite.deletions,
        },
      };
    });
    for (const [index, inputPath] of readCases.entries()) {
      // 各写法都指向同一文件：同会话重复 Read 会得到「未变」提示（TS 同，见 rust-file-tool-results.md），
      // 这里比较正文，所以每种写法用独立会话。
      const rust = await d.call("Read", { file_path: inputPath }, `lexical-${index}`);
      assert.equal(rust.error, undefined, `Read ${inputPath}: ${rust.error}`);
      assert.equal(
        portable(relative(d.root, rust.data.filePath)),
        oracle.reads[index]!.filePath,
        `Read ${inputPath}`,
      );
      assert.equal(rust.data.content, oracle.reads[index]!.content, `Read ${inputPath} content`);
    }
    // 目录软链接（Windows 用无需特权的 junction）：请求路径原样回显，不解析到真实目标。
    try {
      await symlink(join(d.root, "sub"), mirror, process.platform === "win32" ? "junction" : "dir");
      const rust = await d.call("Read", { file_path: linked });
      assert.equal(portable(relative(d.root, rust.data.filePath)), oracle.mirrorPath);
      assert.equal(rust.data.content, "hello\n");
      const rustWrite = await d.call("Write", { file_path: linkedWritten, content: "via link\n" });
      assert.equal(portable(relative(d.root, rustWrite.data.filePath)), oracle.linkedWritePath);
      assert.equal(await readFile(join(d.root, "sub", "linked.txt"), "utf8"), "via link\n");
    } catch (error) {
      // 权限不足以创建链接时只跳过这一子用例，其余词法断言仍必须成立。
      t.diagnostic(`link case skipped: ${error}`);
    }
    // 相对路径写入（含 `..`）的 `filePath` 与结构化输出必须与 TS 同形。
    const rustWrite = await d.call("Write", { file_path: relativeWrite, content: "written\n" });
    // TS write.ts 返回 `filePath: file_path`（模型给出的原样路径，docs/specs/rust-file-tool-results.md）；
    // 下方 TS 对照以解析后的路径调用，所以回显解析后的路径。
    assert.equal(rustWrite.data.filePath, relativeWrite);
    for (const key of ["type", "content", "additions", "deletions"] as const)
      assert.equal(rustWrite.data[key], oracle.write[key], `Write ${key}`);
    assert.equal(oracle.write.filePath, "sub/ts-written.txt");
  } finally {
    await d.close();
  }
});

test("Rust tool definitions use frozen TS schemas and real TS file/search handlers agree on representative calls", async () => {
  const d = await driver();
  try {
    const searchCases = [
      ["Glob", { pattern: "*.txt" }],
      ["Grep", { pattern: "hello", output_mode: "content" }],
      ["Grep", { pattern: "first\\nhello", multiline: true }],
      ["Grep", { pattern: "first|last", output_mode: "content", offset: 1, head_limit: 1 }],
    ] as const;
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
    const oracle = await nodeOracle("tools", async () => {
      const { NodeFileSystemAdapter } = await import(fsAdapterModule);
      const { readTextFileForModel } = await import(readTextModule);
      const { globToolEntry } = await import(globHandlerModule);
      const { grepToolEntry } = await import(grepHandlerModule);
      const { writeToolEntry } = await import(writeHandlerModule);
      const { editToolEntry } = await import(editHandlerModule);
      const { skillToolEntry } = await import(skillHandlerModule);
      const { todoReadToolEntry, todoWriteToolEntry } = await import(todoHandlerModule);
      const { askUserQuestionToolEntry } = await import(askHandlerModule);
      const contracts = await Promise.all([
        import(`${contractsDir}/agent.js`),
        import(`${contractsDir}/send-message.js`),
        import(`${contractsDir}/skill.js`),
        import(`${contractsDir}/todo.js`),
        import(`${contractsDir}/read.js`),
        import(`${contractsDir}/bash.js`),
        import(`${contractsDir}/task-output.js`),
        import(`${contractsDir}/task-stop.js`),
        import(`${contractsDir}/ask-user-question.js`),
        import(`${contractsDir}/plan-mode.js`),
        import(`${contractsDir}/webfetch.js`),
        import(`${contractsDir}/websearch.js`),
        import(`${contractsDir}/read-session-context.js`),
        import(`${contractsDir}/automation.js`),
        import(`${contractsDir}/list-saved-workflows.js`),
        import(`${contractsDir}/list-models.js`),
        import(`${contractsDir}/list-workflow-runs.js`),
        import(`${contractsDir}/create-workflow.js`),
        import(`${contractsDir}/save-workflow.js`),
        import(`${contractsDir}/get-workflow-run.js`),
        import(`${contractsDir}/eval-workflow-snippet.js`),
        import(`${contractsDir}/amend-workflow.js`),
        import(`${contractsDir}/resume-workflow-run.js`),
        import(`${contractsDir}/resolve-workflow-question.js`),
        import(`${contractsDir}/fill-workflow-hole.js`),
      ]);
      const [
        agent,
        sendMessage,
        skill,
        todo,
        read,
        bash,
        taskOutput,
        taskStop,
        askUserQuestion,
        planMode,
        webFetch,
        webSearch,
        readSessionContext,
        automation,
        listSavedWorkflows,
        listModels,
        listWorkflowRuns,
        createWorkflow,
        saveWorkflow,
        getWorkflowRun,
        evalWorkflowSnippet,
        amendWorkflow,
        resumeWorkflowRun,
        resolveWorkflowQuestion,
        fillWorkflowHole,
      ] = contracts as any[];
      const schemas = {
        Agent: agent.AgentInputJsonSchema,
        SendMessage: sendMessage.SendMessageInputJsonSchema,
        Skill: skill.SkillInputJsonSchema,
        TodoRead: todo.TodoReadInputJsonSchema,
        TodoWrite: todo.TodoWriteInputJsonSchema,
        Read: read.ReadInputJsonSchema,
        Bash: bash.BashInputJsonSchema,
        TaskOutput: taskOutput.TaskOutputInputJsonSchema,
        TaskStop: taskStop.TaskStopInputJsonSchema,
        AskUserQuestion: askUserQuestion.AskUserQuestionInputJsonSchema,
        // plan 模式工具（docs/specs/rust-plan-mode.md）。
        EnterPlanMode: planMode.EnterPlanModeInputJsonSchema,
        ExitPlanMode: planMode.ExitPlanModeInputJsonSchema,
        WebFetch: webFetch.WebFetchInputJsonSchema,
        WebSearch: webSearch.WebSearchInputJsonSchema,
        ReadSessionContext: readSessionContext.ReadSessionContextInputJsonSchema,
        CronCreate: automation.CronCreateInputJsonSchema,
        CronList: automation.CronListInputJsonSchema,
        CronUpdate: automation.CronUpdateInputJsonSchema,
        CronDelete: automation.CronDeleteInputJsonSchema,
        ListSavedWorkflows: listSavedWorkflows.ListSavedWorkflowsInputJsonSchema,
        ListModels: listModels.ListModelsInputJsonSchema,
        ListWorkflowRuns: listWorkflowRuns.ListWorkflowRunsInputJsonSchema,
        CreateWorkflow: createWorkflow.CreateWorkflowInputJsonSchema,
        SaveWorkflow: saveWorkflow.SaveWorkflowInputJsonSchema,
        GetWorkflowRun: getWorkflowRun.GetWorkflowRunInputJsonSchema,
        EvalWorkflowSnippet: evalWorkflowSnippet.EvalWorkflowSnippetInputJsonSchema,
        AmendWorkflow: amendWorkflow.AmendWorkflowInputJsonSchema,
        ResumeWorkflowRun: resumeWorkflowRun.ResumeWorkflowRunInputJsonSchema,
        ResolveWorkflowQuestion: resolveWorkflowQuestion.ResolveWorkflowQuestionInputJsonSchema,
        // v3.15.1：留白补全。
        FillWorkflowHole: fillWorkflowHole.FillWorkflowHoleInputJsonSchema,
        Write: writeToolEntry.inputSchema,
        Edit: editToolEntry.inputSchema,
        Glob: globToolEntry.inputSchema,
        Grep: grepToolEntry.inputSchema,
      } as Record<string, unknown>;
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
      };
      await writeFile(join(d.root, "a.txt"), "first\nhello 世界\nlast\n");
      const reads = [];
      for (const args of [{}, { offset: 2, limit: 1 }, { offset: 99 }]) {
        const ts = await readTextFileForModel({
          filePath: join(d.root, "a.txt"),
          fileSystemPort: adapter,
          ...args,
        });
        reads.push({
          content: ts.content,
          startLine: ts.startLine,
          numLines: ts.numLines,
          totalLines: ts.totalLines,
        });
      }
      const entries: Record<string, any> = {
        Glob: globToolEntry,
        Grep: grepToolEntry,
      };
      const search = [];
      for (const [name, args] of searchCases) {
        const ts = (await entries[name]!.handler(args, context)) as any;
        search.push(
          scrub(
            {
              filenames: ts.filenames,
              content: ts.content,
              numFiles: ts.numFiles,
              truncated: ts.truncated,
            },
            d.root,
          ),
        );
      }
      return {
        schemas,
        descriptions: {
          TodoRead: todoReadToolEntry.metadata.description,
          TodoWrite: todoWriteToolEntry.metadata.description,
          AskUserQuestion: askUserQuestionToolEntry.metadata.description,
          Skill: skillToolEntry.metadata.description,
        },
        reads,
        search,
      };
    });
    const schemas = oracle.schemas as Record<string, unknown>;
    const descriptions = oracle.descriptions as Record<string, unknown>;
    for (const { function: tool } of (await d.call("definitions", {})).definitions) {
      assert.deepEqual(canonical(tool.parameters), schemas[tool.name]);
      if (tool.name in descriptions) assert.equal(tool.description, descriptions[tool.name]);
    }
    await writeFile(join(d.root, "a.txt"), "first\nhello 世界\nlast\n");
    for (const [index, args] of [{}, { offset: 2, limit: 1 }, { offset: 99 }].entries()) {
      const rust = await d.call("Read", { file_path: join(d.root, "a.txt"), ...args });
      const expected = oracle.reads[index] as Record<string, unknown>;
      for (const key of ["content", "startLine", "numLines", "totalLines"] as const)
        assert.equal(
          canonical(rust.data[key]),
          expected[key],
          `Read ${key} ${JSON.stringify(args)}`,
        );
    }
    for (const [index, [name, args]] of searchCases.entries()) {
      const rust = await d.call(name, args);
      const observed = scrub(
        {
          filenames: rust.data.filenames,
          content: rust.data.content,
          numFiles: rust.data.numFiles,
          truncated: rust.data.truncated,
        },
        d.root,
      );
      const expected = oracle.search[index] as Record<string, unknown>;
      for (const key of ["filenames", "content", "numFiles", "truncated"] as const)
        assert.deepEqual(canonical(observed[key]), expected[key], `${name} ${key}`);
    }
    // 编辑/宽松匹配的 TS 结果单独录制：TS 会在工作区留下 *.txt，不能影响上面的 Glob/Grep 观察。
    const edits = await nodeOracle("edits", async () => {
      const { NodeFileSystemAdapter } = await import(fsAdapterModule);
      const { writeToolEntry } = await import(writeHandlerModule);
      const { editToolEntry } = await import(editHandlerModule);
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
      };
      const tsPath = join(d.root, "ts.txt");
      const tsWrite = (await writeToolEntry.handler(
        { file_path: tsPath, content: "alpha\nbeta\n" },
        context as any,
      )) as any;
      const replaceAllResults = [];
      for (const replaceAll of [false, true]) {
        const args = {
          old_string: replaceAll ? "alpha" : "beta",
          new_string: replaceAll ? "new" : "alpha",
          replace_all: replaceAll,
        };
        const ts = (await editToolEntry.handler(
          { file_path: tsPath, ...args },
          context as any,
        )) as any;
        replaceAllResults.push({
          finalContent: await readFile(tsPath, "utf8"),
          output: canonical(normalizeEditOutput(ts)),
        });
      }
      const fuzzy = [];
      for (const [index, [initial, args]] of fuzzyCases.entries()) {
        const tsFile = join(d.root, `fuzzy-ts-${index}.txt`);
        // 经 TS Write 建立文件，记下读取状态（编辑前必须已读）。
        await writeToolEntry.handler({ file_path: tsFile, content: initial }, context as any);
        const ts = (await editToolEntry.handler(
          { file_path: tsFile, ...args },
          context as any,
        )) as any;
        fuzzy.push({
          failed: ts.result === false,
          matchStrategy: ts.matchStrategy ?? null,
          message: ts.message ?? null,
          finalContent: await readFile(tsFile, "utf8"),
          output: ts.result === false ? null : canonical(normalizeEditOutput(ts)),
        });
      }
      return { write: { type: tsWrite.type }, replaceAll: replaceAllResults, fuzzy };
    });
    const rustPath = join(d.root, "rust.txt");
    const nativeWrite = await d.call("Write", { file_path: rustPath, content: "alpha\nbeta\n" });
    assert.equal(nativeWrite.data.type, edits.write.type);
    for (const [index, replaceAll] of [false, true].entries()) {
      const args = {
        old_string: replaceAll ? "alpha" : "beta",
        new_string: replaceAll ? "new" : "alpha",
        replace_all: replaceAll,
      };
      const native = await d.call("Edit", { file_path: rustPath, ...args });
      const expected = edits.replaceAll[index]!;
      assert.equal(await readFile(rustPath, "utf8"), expected.finalContent);
      assert.deepEqual(canonical(normalizeEditOutput(native.data)), expected.output);
    }
    // 宽松匹配用例：文件内容与结果逐项比对；每个用例的预期结果（策略名或失败）已冻结，防止空转。
    for (const [index, [initial, args]] of fuzzyCases.entries()) {
      const rustFile = join(d.root, `fuzzy-rust-${index}.txt`);
      // 经 Rust Write 建立文件，两侧都记下读取状态（编辑前必须已读）。
      await d.call("Write", { file_path: rustFile, content: initial });
      const native = await d.call("Edit", { file_path: rustFile, ...args });
      const expected = edits.fuzzy[index]!;
      const label = `fuzzy case ${index} ${JSON.stringify(args)}`;
      assert.equal(await readFile(rustFile, "utf8"), expected.finalContent, label);
      if (expected.failed) {
        assert.ok(native.error, `${label}: Rust should fail like TS (${expected.message})`);
        assert.ok(
          String(native.error).endsWith(String(expected.message)),
          `${label}: ${native.error}`,
        );
        continue;
      }
      assert.equal(native.error, undefined, `${label}: ${native.error}`);
      assert.deepEqual(canonical(normalizeEditOutput(native.data)), expected.output, label);
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
        assert.equal(native.data.status, status);
      }
    }
  } finally {
    await d.close();
  }
});
