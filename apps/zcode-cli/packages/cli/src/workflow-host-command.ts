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
 *   - `tool.execute {session, cwd, tool, input, callId}` → `{content, data}` 或 `{content, isError, handlerFailure}`：
 *     validateInput + handler + formatModelContent。输出带 `backgroundTaskId` 时交给该会话的 TS 后台追踪器
 *     （BackgroundTaskTracker + 运行时任务注册表），结算后发 `runSettled` 通知。
 *   - `tool.cancel {callId}`：中止在飞的 handler。
 *   - `session.close {session}`：停下该会话名下在飞的 run 并释放服务。
 * - 宿主 → Rust 通知 `{event, params}`：`runSettled {session, taskId, toolCallId, text, originMeta}`、
 *   `runNotice {session, taskId, noticeId, text, originMeta}`（run 中的升级问答 / 停滞通知）
 *   （完成通知与 Node runtime 逐字相同，见 core `formatWorkflowTaskNotificationText`）。
 *
 * actor（M2）：远程 AgentRuntime（workflow-host-actors.ts）——每一轮由 Rust 的 actor 子会话执行。宿主经
 * `{event: "request", id, method, params}` 向 Rust 发请求（actor.create / actor.turn / …），Rust 以
 * `{replyTo, result | error}` 应答；Rust 的 actor.tool / actor.event 走普通请求通道进来。
 * run 产物 store 尚未接入。
 */

import { createInterface } from "node:readline";
import { dirname, join } from "node:path";
import { tmpdir } from "node:os";
import { DatabaseSync } from "node:sqlite";
import { createNodeExecutionAdapter } from "@zcode/adapters/exec";
import { createNodeFileSystemAdapter } from "@zcode/adapters/fs";
import { createDwfJournalStore, createNodeToolArtifactStore } from "@zcode/adapters/storage";
import { runWorkflowQuery } from "./workflow-host-queries.js";
import { createHookHost } from "./workflow-host-hooks.js";
import {
  createDynamicWorkflowRunService,
  getWorkflowConcurrencyGovernor,
  workflowLifecycleFactFromProgress,
} from "@zcode/bootstrap";
import { isAmendWorkflowOwnedPredecessor } from "@zcode/contracts";
import { createActorBridge } from "./workflow-host-actors.js";
import { isWorkflowRunCommand, runWorkflowRunCommand, type WorkflowRunCommandDeps } from "./workflow-host-runs.js";
import {
  clampWorkflowRunsForLegacy,
  diffWorkflowRunsState,
  reduceWorkflowRunsState,
  type WorkflowRunsState,
} from "@zcode/shared/zcode-protocol-v4";
import {
  BackgroundTaskTracker,
  buildWorkflowRunProgressNotification,
  builtInTools,
  InMemoryRuntimeTaskRegistry,
  isTerminalRuntimeTask,
} from "@zcode/core";
import { createSavedWorkflowLaunch } from "./workflow-host-saved-start.js";

export const ZCODE_WORKFLOW_HOST_COMMAND = "__zcode-workflow-host";

/** 宿主负责的工作流工具（其余工具仍由 Rust 原生实现）。 */
const HOST_TOOLS = new Set([
  "CreateWorkflow",
  "AmendWorkflow",
  "ResumeWorkflowRun",
  "GetWorkflowRun",
  "ResolveWorkflowQuestion",
  // 只在 task_id 指向工作流 run 时由 Rust 转来（TS 后台任务控制端口的 local_dynamic_workflow 分支
  // 与运行时任务注册表）。
  "TaskStop",
  "TaskOutput",
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
  /** run 用户面产物（`artifact.file` / `artifact.markdown`）的字节落点：Rust 数据目录下，布局同 Node 的 cli/artifacts。 */
  let artifactStore: ReturnType<typeof createNodeToolArtifactStore> | undefined;
  const services = new Map<string, Service>();
  const inflight = new Map<string, AbortController>();
  const executionPort = createNodeExecutionAdapter({
    outputRootDir: join(tmpdir(), "zcode-workflow-host-exec"),
    processEnv: process.env,
  });
  const fileSystemPort = createNodeFileSystemAdapter();
  const send0 = (message: unknown): void => {
    process.stdout.write(`${JSON.stringify(message)}
`);
  };
  const hooks = createHookHost(send0, executionPort);
  const send = (message: unknown): void => {
    process.stdout.write(`${JSON.stringify(message)}\n`);
  };
  // 宿主 → Rust 的请求（actor 会话操作）：按 id 等应答。
  let nextRustId = 0;
  const rustPending = new Map<string, { resolve: (value: any) => void; reject: (error: Error) => void }>();
  const rustRequest = (method: string, params: Record<string, unknown>): Promise<any> =>
    new Promise((resolve, reject) => {
      const id = `h${++nextRustId}`;
      rustPending.set(id, { resolve, reject });
      send({ event: "request", id, method, params });
    });
  /** actor 会话 → driver 下发的模型请求准入端口；票据 id → 已准入的票据。 */
  const admissions = new Map<string, any>();
  const tickets = new Map<string, any>();
  let nextTicket = 0;
  /** actor 会话 → 它所属会话的桥（actor.tool / actor.event 据此路由）。 */
  const actorBridges = new Map<string, ReturnType<typeof createActorBridge>>();
  /** 每个父会话最近一次工作流工具调用带来的模型选择（actor 的模型基线）。 */
  const selections = new Map<string, Record<string, unknown>>();
  /**
   * 每个父会话一份 TS 运行时任务注册表与后台追踪器（Node 里它们属于该会话的 AgentRuntime）：run 的登记、
   * 结算、resultText、通知认领（TaskOutput 读到终态即认领）与 superseded 抑制都走 TS 原实现。
   */
  const trackers = new Map<string, { registry: InMemoryRuntimeTaskRegistry; tracker: BackgroundTaskTracker }>();
  const workingDirectories = new Map<string, string>();
  /** 父会话 → V4 `workflowRuns` 归约态（宿主进程内；Rust 持久化每次的整键结果）。 */
  const runStates = new Map<string, WorkflowRunsState>();
  /** V4 `workflowRuns` 状态键：与 Node 投影同一个归约（@zcode/shared），整键交给 Rust 进会话快照。 */
  const reduceRuns = (owner: string, progress: unknown): void => {
    const prior = runStates.get(owner);
    const workflowRuns = reduceWorkflowRunsState(prior, progress as never);
    if (workflowRuns === null) return;
    runStates.set(owner, workflowRuns);
    // 没有 `workflowRunDeltas` 能力的订阅者收旧界裁剪版（TS publisher 的旧消费者编码）。
    const legacy = clampWorkflowRunsForLegacy(workflowRuns);
    send({
      event: "workflowRuns",
      params: {
        session: owner,
        kind: "workflowRuns",
        workflowRuns,
        // 键级增量（TS projection diffWorkflowRunsState）：有 `workflowRunDeltas` 能力的订阅者收它而不是整键。
        deltas: diffWorkflowRunsState(prior, workflowRuns),
        ...(legacy === workflowRuns ? {} : { legacy }),
      },
    });
  };
  const runChains = new Map<string, Promise<void>>();
  const tracking = (session: string) => {
    let existing = trackers.get(session);
    if (existing === undefined) {
      const registry = new InMemoryRuntimeTaskRegistry();
      const tracker = new BackgroundTaskTracker({
        runtimeTaskRegistry: registry,
        sessionId: session,
        dynamicWorkflowRunPort: service(session),
        getWorkingDirectory: () => workingDirectories.get(session) ?? process.cwd(),
        emitEvent: async () => {},
        enqueueBackgroundTaskNotification: (notification: Record<string, any>) => {
          send({
            event: "runSettled",
            params: {
              session,
              taskId: notification.taskId,
              text: notification.text,
              originMeta: notification.originMeta,
            },
          });
        },
      } as never);
      existing = { registry, tracker };
      trackers.set(session, existing);
    }
    return existing;
  };
  /** TS runtime.stopBackgroundTask 的 local_dynamic_workflow 分支（background.ts / background-stop-dynamic-workflow.ts）。 */
  const backgroundTaskControlPort = (session: string) => ({
    stopBackgroundTask: async (taskId: string, options: { initiator?: "user" | "model"; strict?: boolean }) => {
      const type = "local_dynamic_workflow";
      const { registry } = tracking(session);
      const task = registry.get(taskId);
      if (task === undefined) return { ok: false, reason: "background_task_not_found", taskId };
      if (isTerminalRuntimeTask(task)) {
        return options.strict === true
          ? { ok: false, reason: "background_task_not_running", status: task.status, taskId, type }
          : { alreadyTerminal: true, ok: true, status: task.status, taskId, type };
      }
      if (options.initiator !== undefined) {
        registry.update(taskId, (current) => ({ ...current, stopInitiator: options.initiator }));
      }
      const cancelled = await service(session).cancel(taskId, options.initiator as never);
      return cancelled
        ? { ok: true, status: "cancelled", taskId, type }
        : { ok: false, reason: "background_task_not_found", status: "lost", taskId, type };
    },
  });
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
        ...(artifactStore === undefined ? {} : { artifactStore }),
        // run 中通知（升级问答 / 停滞）：与 Node runtime 的进度汇同一个格式器，交给 Rust 作为后台结果轮。
        onRunEvent: (progress, routing) => {
          // V4 `workflowRuns` 状态键：与 Node 投影同一个归约（@zcode/shared），整键交给 Rust 进会话快照。
          const owner = routing.parentSessionId ?? session;
          // 本宿主进程内首次见到该会话时先向 Rust 取已持久化的归约态（宿主重启不丢此前的 run），
          // 同一会话的后续事件按序排在它之后。
          const chained = (runChains.get(owner) ?? Promise.resolve()).then(async () => {
            if (!runStates.has(owner)) {
              const seeded = await rustRequest("workflowRuns.prior", { session: owner }).catch(() => null);
              if (seeded && !runStates.has(owner)) runStates.set(owner, seeded as WorkflowRunsState);
            }
            reduceRuns(owner, progress);
          });
          runChains.set(owner, chained.catch(() => {}));
          // `workflow.lifecycle` 遥测事实（actor-spawned / run-settled）：基字段由 Rust 重新盖章。
          const fact = workflowLifecycleFactFromProgress(
            { version: 1, eventId: "host", eventSeq: 0, occurredAt: Date.now(), sessionId: owner },
            progress,
          ) as Record<string, unknown> | null;
          if (fact !== null) {
            const { version: _v, eventId: _e, eventSeq: _s, occurredAt: _o, sessionId: _id, ...fields } = fact;
            send({ event: "workflowRuns", params: { session: owner, kind: "telemetry", fact: fields } });
          }
          const runLabel = tracking(session).registry.get(progress.runId)?.description ?? progress.runId;
          const notification = buildWorkflowRunProgressNotification(progress, runLabel);
          if (notification === undefined) return;
          const noticeId = String(progress.payload.qid ?? `${progress.eventType}:${progress.sequence}`);
          send({
            event: "runNotice",
            params: { session: routing.parentSessionId ?? session, taskId: progress.runId, noticeId, ...notification },
          });
        },
        // 进程级并发治理器（TS create-app 同一实例）：actor 的模型请求经 `actor.admission.*` 过它的闸门。
        concurrency: getWorkflowConcurrencyGovernor(),
        createActorRuntime: (() => {
          const bridge = createActorBridge(rustRequest, session, () => selections.get(session));
          return (input: Record<string, any>) => {
            actorBridges.set(String(input.sessionId), bridge);
            if (input.modelRequestAdmission !== undefined) admissions.set(String(input.sessionId), input.modelRequestAdmission);
            return bridge.createActorRuntime(input);
          };
        })() as never,
      });
      services.set(session, existing);
    }
    return existing;
  };
  const toolContext = (params: Record<string, any>, signal?: AbortSignal) => ({
    workingDirectory: params.cwd as string,
    dynamicWorkflowRunPort: service(params.session),
    backgroundTaskControlPort: backgroundTaskControlPort(params.session),
    runtimeTaskRegistry: tracking(params.session).registry,
    sessionId: params.session,
    hasLoadedSkill: (name: string) => params.skillLoaded === true && name === DYNAMIC_WORKFLOW_SKILL,
    ...(params.callId === undefined ? {} : { toolCallId: params.callId }),
    ...(signal === undefined ? {} : { abortSignal: signal }),
  });

  // 中枢「运行」的两段（startSaved / trackSaved）拆在独立模块：宿主命令面已接近单文件 400 行上限。
  const savedLaunch = createSavedWorkflowLaunch({
    port: service,
    track: (session, toolCall, runId) =>
      tracking(session).tracker.trackBackgroundTask(
        toolCall as never,
        { backgroundTaskId: runId, status: "backgrounded" },
        // 与 run.resume 的重臂同一条（trace 取合成工具调用 id）。
        { traceId: String(toolCall.id) } as never,
        undefined,
      ),
  });
  const runDeps: WorkflowRunCommandDeps = {
    ...savedLaunch,
    resume: async (session, runId) => {
      const port = service(session);
      if (typeof port.resume !== "function") throw new Error("Workflow run resume is unavailable");
      return (await port.resume(runId)) as Record<string, unknown>;
    },
    stop: (session, runId) => backgroundTaskControlPort(session).stopBackgroundTask(runId, { initiator: "user" }),
    track: (session, toolCall, output) =>
      tracking(session).tracker.trackBackgroundTask(toolCall as never, output, { traceId: toolCall.id } as never, undefined),
    setWorkingDirectory: (session, cwd) => workingDirectories.set(session, cwd),
    onTrackError: (error) =>
      send({ event: "hostError", params: { message: error instanceof Error ? error.message : String(error) } }),
  };

  const handle = async (request: Request): Promise<unknown> => {
    const params = request.params ?? {};
    if (typeof params.session === "string" && params.selection !== undefined) {
      selections.set(params.session, params.selection);
    }
    switch (request.method) {
      case "init": {
        const db = new DatabaseSync(params.dbPath as string);
        db.exec("PRAGMA busy_timeout = 5000; PRAGMA foreign_keys = ON;");
        journal = createDwfJournalStore(db);
        const root = dirname(params.dbPath as string);
        artifactStore = createNodeToolArtifactStore({
          imageCacheRootDir: join(root, "workflow-artifacts", "image-cache"),
          pdfCacheRootDir: join(root, "workflow-artifacts", "pdf-cache"),
          rootDir: join(root, "workflow-artifacts", "artifacts"),
          videoCacheRootDir: join(root, "workflow-artifacts", "video-cache"),
        });
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
        // TS 权限服务的 workflowOwner 规则：修订本会话发起、且非用户停下的 run 免确认（阻断规则照常生效）。
        const owned = params.tool === "AmendWorkflow" && isAmendWorkflowOwnedPredecessor(input?.predecessor);
        return { input, ask: gate.gate === "ask" && !owned };
      }
      case "tool.execute": {
        const tool = entry(params.tool);
        const controller = new AbortController();
        inflight.set(params.callId, controller);
        workingDirectories.set(params.session, params.cwd);
        try {
          // TS executor 的 validateInput（TaskOutput 的「无此任务」等）：可修复失败，按工具失败回给模型。
          const validation = tool.validateInput?.(params.input, {
            runtimeTaskRegistry: tracking(params.session).registry,
          } as never);
          if (validation && validation.result === false) {
            return { content: validation.message, isError: true, handlerFailure: true };
          }
          const output = (await tool.handler(
            params.input,
            toolContext(params, controller.signal) as never,
          )) as Record<string, unknown>;
          const content = tool.formatModelContent ? tool.formatModelContent(output) : JSON.stringify(output);
          // 后台 run：TS tracker 登记、等结算、发完成通知（与 Node 的 call-runner → trackBackgroundTask 同路）。
          const toolCall = { id: params.callId, name: params.tool, input: params.input };
          void tracking(params.session)
            .tracker.trackBackgroundTask(toolCall as never, output, { traceId: params.callId } as never, undefined)
            .catch((error: unknown) => {
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
      // hooks（docs/specs/rust-hooks.md）：按调用点执行，callId 可被 tool.cancel 中止。
      case "hooks.run": {
        const controller = new AbortController();
        if (params.callId !== undefined) inflight.set(params.callId, controller);
        try {
          return await hooks.run(params, controller.signal);
        } finally {
          if (params.callId !== undefined) inflight.delete(params.callId);
        }
      }
      // 工作区 hooks 的审核命令与无会话授权（H2）。
      case "hooks.review":
        return hooks.review(params);
      case "hooks.trustGrant":
        return hooks.trustGrant(params);
      // V4 工作流只读查询（`v4/conversation/workflowRun*`）。
      case "v4.query":
        return runWorkflowQuery(params.method, params.params, (session) => service(session) as never);
      case "actor.tool": {
        const bridge = actorBridges.get(params.actorSession);
        if (bridge === undefined) return { content: "Unknown workflow actor session", isError: true };
        return bridge.handleTool(params);
      }
      // actor 模型请求的准入（Rust 每次尝试前取票，状态事件依序投入，结束释放）。
      case "actor.admission.acquire": {
        const admission = admissions.get(params.actorSession);
        if (admission === undefined) return { ticket: null };
        const model = { providerId: params.providerId, modelId: params.modelId };
        const ticket = admission.tryAcquire?.({ model }) ?? (await admission.acquire({ model }));
        const id = `t${++nextTicket}`;
        tickets.set(id, ticket);
        return { ticket: id };
      }
      case "actor.admission.publish":
        await tickets.get(params.ticket)?.publish(params.event);
        return { ok: true };
      case "actor.admission.release":
        tickets.get(params.ticket)?.release();
        tickets.delete(params.ticket);
        return { ok: true };
      case "actor.event":
        actorBridges.get(params.actorSession)?.handleEvent(params);
        return { ok: true };
      case "tool.cancel":
        inflight.get(params.callId)?.abort();
        return { ok: true };
      case "session.close": {
        hooks.close(params.session);
        const existing = services.get(params.session);
        services.delete(params.session);
        await existing?.close();
        return { ok: true };
      }
      default:
        // 用户命令面的 run 取消 / 恢复（workflow-host-runs.ts）。
        if (isWorkflowRunCommand(request.method)) return runWorkflowRunCommand(request.method, params, runDeps);
        throw new Error(`Unsupported workflow host request: ${request.method}`);
    }
  };

  const lines = createInterface({ input: process.stdin, crlfDelay: Infinity });
  for await (const line of lines) {
    if (line.trim().length === 0) continue;
    let request: Request & { replyTo?: string; result?: unknown; error?: string };
    try {
      request = JSON.parse(line) as typeof request;
    } catch {
      continue;
    }
    // Rust 对宿主请求的应答。
    if (request.replyTo !== undefined) {
      const pending = rustPending.get(request.replyTo);
      rustPending.delete(request.replyTo);
      if (request.error !== undefined) pending?.reject(new Error(request.error));
      else pending?.resolve(request.result);
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
