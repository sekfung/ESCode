/**
 * 隐藏子命令 `__zcode-workflow-host`：Rust runtime 的动态工作流宿主（docs/specs/rust-dynamic-workflow.md
 * 「运行面实现计划」）。每个 Rust 进程一个、常驻；复用 TS 的 run 服务与工作流工具 handler，语义因此与
 * Node runtime 一致。journal 用 TS 仓储直接写 Rust 会话库的 dwf_* 表（库与表结构归 Rust）。
 *
 * 协议（每行一个 JSON）：
 * - Rust → 宿主请求 `{id, method, params}`，宿主回 `{id, result}` 或 `{id, error}`（按 id 多路复用、可并发）：
 *   - `init {dbPath}`：打开 Rust 会话库（只碰 dwf_* 表）。
 *   - `tool.prepare {session, cwd, tool, input, skillLoaded}` → `{rejected: message}` 或 `{input, ask}`：
 *     TS executor 的 validateInput → resolveInput → prepareApproval 三段。
 *   - `tool.execute {session, cwd, tool, input, callId}` → `{content, data}`：handler + formatModelContent。
 *     输出带 `backgroundTaskId` 时宿主接着等这个 run 结算，再发 `runSettled` 通知。
 *   - `tool.cancel {callId}`：中止在飞的 handler。
 *   - `session.close {session}`：停下该会话名下在飞的 run 并释放服务。
 * - 宿主 → Rust 通知 `{event, params}`：`runSettled {session, taskId, toolCallId, text, originMeta}`
 *   （完成通知与 Node runtime 逐字相同，见 core `formatWorkflowTaskNotificationText`）。
 *
 * M1：actor（`agent()`）在 Rust 侧接管之前以命名失败结束；run 产物 store 尚未接入。
 */

import { createInterface } from "node:readline";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { DatabaseSync } from "node:sqlite";
import { createNodeExecutionAdapter } from "@zcode/adapters/exec";
import { createNodeFileSystemAdapter } from "@zcode/adapters/fs";
import { createDwfJournalStore } from "@zcode/adapters/storage";
import { createDynamicWorkflowRunService } from "@zcode/bootstrap";
import {
  buildWorkflowNotificationOriginMeta,
  builtInTools,
  formatWorkflowTaskNotificationText,
} from "@zcode/core";

export const ZCODE_WORKFLOW_HOST_COMMAND = "__zcode-workflow-host";

/** 宿主负责的工作流工具（其余工具仍由 Rust 原生实现）。 */
const HOST_TOOLS = new Set([
  "CreateWorkflow",
  "AmendWorkflow",
  "ResumeWorkflowRun",
  "GetWorkflowRun",
  "ResolveWorkflowQuestion",
]);
const DYNAMIC_WORKFLOW_SKILL = "dynamic-workflows";

type Service = ReturnType<typeof createDynamicWorkflowRunService>;
interface Request {
  id: number;
  method: string;
  params: Record<string, any>;
}

export async function runWorkflowHostCommand(): Promise<number> {
  let journal: ReturnType<typeof createDwfJournalStore> | undefined;
  const services = new Map<string, Service>();
  const inflight = new Map<string, AbortController>();
  const executionPort = createNodeExecutionAdapter({
    outputRootDir: join(tmpdir(), "zcode-workflow-host-exec"),
    processEnv: process.env,
  });
  const fileSystemPort = createNodeFileSystemAdapter();
  const send = (message: unknown): void => {
    process.stdout.write(`${JSON.stringify(message)}\n`);
  };
  const entry = (name: string) => {
    const found = builtInTools.find((tool) => tool.metadata.name === name);
    if (found === undefined || !HOST_TOOLS.has(name)) throw new Error(`Unknown workflow tool: ${name}`);
    return found;
  };
  const service = (session: string): Service => {
    if (journal === undefined) throw new Error("Workflow host is not initialized");
    let existing = services.get(session);
    if (existing === undefined) {
      existing = createDynamicWorkflowRunService({
        journal,
        parentSessionId: session,
        fileSystemPort,
        executionPort,
        createActorRuntime: () => {
          throw new Error(
            "Workflow actors (agent()) are not available in the Rust runtime yet; run scripts without agent() calls.",
          );
        },
      });
      services.set(session, existing);
    }
    return existing;
  };
  const toolContext = (params: Record<string, any>, signal?: AbortSignal) => ({
    workingDirectory: params.cwd as string,
    dynamicWorkflowRunPort: service(params.session),
    sessionId: params.session,
    hasLoadedSkill: (name: string) => params.skillLoaded === true && name === DYNAMIC_WORKFLOW_SKILL,
    ...(params.callId === undefined ? {} : { toolCallId: params.callId }),
    ...(signal === undefined ? {} : { abortSignal: signal }),
  });

  /** 后台 run 结算后生成完成通知（Node 由 tracker 发出，这里同一个格式器）。 */
  const trackRun = async (params: Record<string, any>, output: Record<string, unknown>) => {
    const taskId = output.backgroundTaskId;
    if (typeof taskId !== "string") return;
    const port = service(params.session);
    const snapshot = await port.waitForTask(taskId);
    const status = snapshot?.status ?? "lost";
    const toolCall = { id: params.callId, name: params.tool, input: params.input } as never;
    const text = formatWorkflowTaskNotificationText({
      toolCall,
      taskId,
      status,
      snapshot: snapshot as never,
      launchOutput: output,
      workingDirectory: params.cwd,
    });
    const originMeta = buildWorkflowNotificationOriginMeta(toolCall, taskId, status, snapshot as never, output);
    send({
      event: "runSettled",
      params: { session: params.session, taskId, toolCallId: params.callId, status, text, originMeta },
    });
  };

  const handle = async (request: Request): Promise<unknown> => {
    const params = request.params ?? {};
    switch (request.method) {
      case "init": {
        const db = new DatabaseSync(params.dbPath as string);
        db.exec("PRAGMA busy_timeout = 5000; PRAGMA foreign_keys = ON;");
        journal = createDwfJournalStore(db);
        return { ok: true };
      }
      case "tool.prepare": {
        const tool = entry(params.tool);
        const validation = tool.validateInput?.(params.input, {} as never);
        if (validation && validation.result === false) return { rejected: validation.message };
        let input = params.input;
        if (tool.resolveInput) {
          const resolution = await tool.resolveInput(input, toolContext(params) as never);
          if (resolution.result === false) return { rejected: resolution.message };
          input = resolution.input;
        }
        const gate = tool.prepareApproval ? tool.prepareApproval(input) : { gate: "ask" as const };
        return { input, ask: gate.gate === "ask" };
      }
      case "tool.execute": {
        const tool = entry(params.tool);
        const controller = new AbortController();
        inflight.set(params.callId, controller);
        try {
          const output = (await tool.handler(
            params.input,
            toolContext(params, controller.signal) as never,
          )) as Record<string, unknown>;
          const content = tool.formatModelContent ? tool.formatModelContent(output) : JSON.stringify(output);
          void trackRun(params, output).catch((error: unknown) => {
            send({
              event: "hostError",
              params: { message: error instanceof Error ? error.message : String(error) },
            });
          });
          return { content, data: output };
        } finally {
          inflight.delete(params.callId);
        }
      }
      case "tool.cancel":
        inflight.get(params.callId)?.abort();
        return { ok: true };
      case "session.close": {
        const existing = services.get(params.session);
        services.delete(params.session);
        await existing?.close();
        return { ok: true };
      }
      default:
        throw new Error(`Unsupported workflow host request: ${request.method}`);
    }
  };

  const lines = createInterface({ input: process.stdin, crlfDelay: Infinity });
  for await (const line of lines) {
    if (line.trim().length === 0) continue;
    let request: Request;
    try {
      request = JSON.parse(line) as Request;
    } catch {
      continue;
    }
    void handle(request).then(
      (result) => send({ id: request.id, result }),
      (error: unknown) =>
        send({ id: request.id, error: error instanceof Error ? error.message : String(error) }),
    );
  }
  await Promise.allSettled([...services.values()].map((existing) => existing.close()));
  await executionPort.close?.();
  return 0;
}
