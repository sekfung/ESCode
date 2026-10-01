/**
 * 隐藏子命令 `__zcode-workflow-analyzer`：Rust runtime 的工作流脚本分析子进程
 * （docs/specs/rust-dynamic-workflow.md 第 3 期，路线 1：附带 Node 跑既有 TS 分析器）。
 *
 * 诊断与类型结论来自 TypeScript 类型检查器，Rust 不复刻它；这里就是同一份
 * `analyzeWorkflowScript`，Rust 经 NDJSON 调用，结果因此与 Node runtime 逐字一致。
 *
 * 协议（每行一个 JSON）：请求 `{id, method: "analyze", script}`，应答 `{id, result}` 或
 * `{id, error}`。`result` 是 `AnalyzeResult` 的 JSON 形：`core` 经 `encodeAnalysisCore`
 * （其中的 Map 不能直接 stringify）；`graph` / `causality` / `flow` / `handoff` 本就是纯数据。
 * 进程常驻、串行处理，stdin 关闭即退出。与 plugin host 同理，在导入 `run` 之前分派。
 */

import { createInterface } from "node:readline";
import { analyzeWorkflowScript, encodeAnalysisCore } from "@zcode/dynamic-workflow";

export const ZCODE_WORKFLOW_ANALYZER_COMMAND = "__zcode-workflow-analyzer";

export function isWorkflowAnalyzerInvocation(argv: readonly string[]): boolean {
  return argv[0] === ZCODE_WORKFLOW_ANALYZER_COMMAND;
}

interface AnalyzerRequest {
  id?: unknown;
  method?: unknown;
  script?: unknown;
}

/** 把一次分析结果转成可 stringify 的纯对象。 */
export function analyzeToJson(script: string): Record<string, unknown> {
  const result = analyzeWorkflowScript(script);
  const { core, ...rest } = result;
  return { ...rest, ...(core ? { core: encodeAnalysisCore(core) } : {}) };
}

export async function runWorkflowAnalyzerCommand(): Promise<number> {
  const lines = createInterface({ input: process.stdin, crlfDelay: Infinity });
  for await (const line of lines) {
    if (line.trim().length === 0) continue;
    let request: AnalyzerRequest;
    try {
      request = JSON.parse(line) as AnalyzerRequest;
    } catch (error) {
      process.stdout.write(`${JSON.stringify({ id: null, error: String(error) })}\n`);
      continue;
    }
    const id = request.id ?? null;
    try {
      if (request.method !== "analyze" || typeof request.script !== "string") {
        throw new Error(`Unsupported analyzer request: ${String(request.method)}`);
      }
      const result = analyzeToJson(request.script);
      process.stdout.write(`${JSON.stringify({ id, result })}\n`);
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      process.stdout.write(`${JSON.stringify({ id, error: message })}\n`);
    }
  }
  return 0;
}
