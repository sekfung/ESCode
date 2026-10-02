/**
 * 隐藏子命令 `__zcode-workflow-snippet`：Rust runtime 的 `EvalWorkflowSnippet` 执行子进程
 * （docs/specs/rust-dynamic-workflow.md，路线 1：脚本执行留在 Node 沙箱）。
 *
 * 一次调用一个进程：stdin 第一行是请求 `{code, cwd, timeoutMs}`，之后若收到 `cancel` 行即中止
 * （abort 信号一路到 harness，kill 沙箱子进程）。stdout 写一行结果后退出：
 * `DynamicWorkflowSnippetEvalResult`，`completed` 时附 `serialized`（`serializeWorkflowArtifact`，
 * 与 Node handler 渲染返回值用的是同一个函数）。执行面就是 Node runtime 的 snippet 服务本身。
 */

import { createInterface } from "node:readline";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { createNodeExecutionAdapter } from "@zcode/adapters/exec";
import { createNodeFileSystemAdapter } from "@zcode/adapters/fs";
import { createDynamicWorkflowSnippetService } from "@zcode/bootstrap";
import { serializeWorkflowArtifact } from "@zcode/contracts";

export const ZCODE_WORKFLOW_SNIPPET_COMMAND = "__zcode-workflow-snippet";

interface SnippetRequest {
  code: string;
  cwd: string;
  timeoutMs: number;
}

export async function runWorkflowSnippetCommand(): Promise<number> {
  const controller = new AbortController();
  const lines = createInterface({ input: process.stdin, crlfDelay: Infinity });
  const iterator = lines[Symbol.asyncIterator]();
  const first = await iterator.next();
  if (first.done) return 1;
  const request = JSON.parse(first.value) as SnippetRequest;
  // 后续行只认 `cancel`：工具调用被取消时 Rust 写这一行，再等进程自己收尾。
  void (async () => {
    for (;;) {
      const next = await iterator.next();
      if (next.done) return;
      if (next.value.trim() === "cancel") controller.abort();
    }
  })();
  const executionPort = createNodeExecutionAdapter({
    outputRootDir: join(tmpdir(), "zcode-workflow-snippet-exec"),
    processEnv: process.env,
  });
  const port = createDynamicWorkflowSnippetService({
    executionPort,
    fileSystemPort: createNodeFileSystemAdapter(),
  });
  try {
    const result = await port.evalSnippet(
      {
        code: request.code,
        cwd: request.cwd,
        timeoutMs: request.timeoutMs,
        trace: { traceId: "rust-runtime" as never, spanId: "eval-workflow-snippet" },
      },
      { signal: controller.signal },
    );
    const serialized =
      result.kind === "completed" ? serializeWorkflowArtifact(result.artifact) : undefined;
    process.stdout.write(
      `${JSON.stringify({ ...result, ...(serialized === undefined ? {} : { serialized }) })}\n`,
    );
    return 0;
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    process.stdout.write(`${JSON.stringify({ kind: "crashed", message })}\n`);
    return 1;
  } finally {
    await executionPort.close?.();
    lines.close();
  }
}
