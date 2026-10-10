import { runtimeExecutionCapabilitiesSchema } from "../execution-state.js";
import {
  databaseStartupErrorCodeSchema,
  databaseStartupErrorDetailsSchema,
  databaseMigrationFactsSchema,
} from "../database-startup.js";
/* oxlint-disable eslint(max-lines) -- ESCode Protocol schema 需要单文件导出，方便 app 与 agent 共享同一份协议契约。 */
// ── 旧协议删除边界──────────────────────
// 剩余 ~257 个导出：旧 ESCode Protocol 方法契约、请求/响应/事件 schema、
// session/workspace state snapshot 投影等（承重类型已迁 escode-protocol-legacy-types.ts）。
// 已连根删除的死词（词表+schema+两侧实现）：session/steer、session/rewind、
// session/rewindCascade、session/previewFileRewind、session/applyFileRewind、
// prompt/enhance 全簇（含 promptEnhanceResult 通知）、plugins/marketplace/list；
// session/fork 客户端链已删（op+schema 留存 = v4 forkSessionAtMessage 钩子消费）。
// 消费者：services 旧栈（escodeProtocolClient/escodeAgent/escodeAgentService/escodeSession*）、
// CLI bootstrap 旧协议 server（escode-protocol/server-operations、plugins、session-mapper 等）、
// UI 旧投影（escodeSessionProjection 等读路径）。
// 上述旧协议 client/server 组删除时，本文件整体删除。
// 注：外部零消费 schema 多为存活 schema 联合的内部依赖，随宿主文件一起处理，勿单删。
import { bashOutputDisplaySchema } from "../bash-output-display.js";
// 后台详情共享精简的只读响应 schema，不携带命令或计时元数据。
export * from "../background-bash-output.js";
import { executionOutputPreviewSchema } from "../execution-output-preview.js";
import { z } from "zod";
export * from "../process-diagnostic.js";
import { errorAttributionSchema } from "../escode-protocol-v4/snapshot.js";
import { modelSelectionSchema } from "../model-selection.js";
import { completeModelPropertiesDataSchema } from "../model-config.js";
import { accountProviderUnavailableReasonSchema } from "../account-provider-state.js";
import { modelExecutionSchema } from "../model-execution.js";
import { APP_USAGE_RANGES, appUsageSnapshotSchema } from "../usage-stats.js";
import { escodeAutomationBotDeliveryTargetSchema } from "../bots.js";
// browser-use 命令/结果契约单一来源：agent 构造、协议校验和 main executor 共用同一 schema。
import { browserClientModeSchema, browserCommandSchema } from "../browser-use/commands.js";
import {
  browserBackendListResultSchema,
  browserSessionContextKindSchema,
} from "../browser-use/backend.js";
import { browserCommandResultSchema } from "../browser-use/result.js";
import { integratedTerminalShellSelectionSchema } from "../validationAppSettings.js";
import { escodeTaskModeSchema } from "../escode-task-mode-schema.js";
import { OFFICIAL_MCP_AUTH_PORT_FAILURE_REASONS } from "../official-mcp-auth.js";
import {
  escodeSerialCancelMethod,
  escodeSerialCancelParamsSchema,
  escodeSerialMethodParamsSchemas,
  escodeSerialMethodResultSchemas,
  escodeSerialMethods,
} from "../serial/serialAgentTools.js";
import {
  escodeDeliveryKindSchema,
  escodeMessageVisibilitySchema,
  escodeSyntheticUserMessageSourceSchema as legacyEscodeSyntheticUserMessageSourceSchema,
  escodeWorkspaceRefSchema,
  escodePermissionDecisionSchema,
  escodePermissionResponseSchema,
  escodePermissionUpdateSchema,
  escodeSessionModeSchema,
  escodeSessionStatusSchema,
  escodeSessionKindSchema,
  escodeSessionGoalSchema,
  escodeSessionGoalVerificationSchema,
  escodeSessionGoalVerificationTimelineSchema,
  escodeInteractionRequestOriginSchema,
  escodeToolStateSchema,
  escodeSessionApiRetryStatusSchema,
  escodeSessionContextUsageSchema,
  escodeSessionInfoSchema,
  escodeSessionRuntimeStateSchema,
  escodeMessageWithPartsSchema,
  escodeMessagePartSchema,
} from "../escode-protocol-legacy-types.js";

export {
  hookExecutionProjectionSchema,
  hookInvocationRowSchema,
  type HookExecutionProjection,
  type HookInvocationRow,
} from "../escode-protocol-v4/rows.js";

export const ESCODE_PROTOCOL_NAME = "ESCode Protocol" as const;
export const ESCODE_PROTOCOL_VERSION = 1 as const;
// V4 wire 与 legacy 主协议并存；禁止为了 V4 physical framing 改写 legacy 版本。
export const ESCODE_PROTOCOL_V4_WIRE_VERSION = 3 as const;
export const escodeRuntimeCapabilitiesSchema = z.object({
  workspaceExecutionCapabilities: z.boolean().optional(),
  independentPlanState: z.boolean().optional(),
  /** 原生核心可显式关闭账号 Overlay；缺省保持现有 Registry 同步行为。 */
  accountProviderConfig: z.boolean().optional(),
});
export const escodeProtocolErrorCodes = {
  sessionUnavailable: -32004,
} as const;

const nonEmptyString = z.string().trim().min(1);
const jsonObjectSchema = z.record(z.string(), z.unknown());
const timestampMsSchema = z.number().int().nonnegative();
const protocolInstantSchema = z.union([timestampMsSchema, nonEmptyString, z.date()]);

// Tool result display 不受模型文本 budget 约束；Node REPL 图片必须在 Agent/App 协议边界
// 做严格限长，避免截图把 continuous 或 replayable 消息扩成无界载荷。
export const escodeNodeReplImageToolResultDisplaySchema = z
  .object({
    kind: z.literal("node_repl_images"),
    images: z
      .array(
        z
          .object({
            base64: z
              .string()
              .min(1)
              .max(200 * 1024),
            mimeType: z.string().regex(/^image\/[a-z0-9.+-]+$/iu),
          })
          .strict(),
      )
      .min(1)
      .max(2),
    truncated: z.boolean().optional(),
    source: z.literal("browser_turn_end").optional(),
  })
  .strict();

// 同理：CreateWorkflow 的类型检查诊断也是 display 通道，必须在协议边界限长，
// 避免大量诊断把 continuous/replayable 消息扩成无界载荷。
// causalityGraph 在工具输出边界已限长，这里镜像同一组上界（与 v4 rows 保持一致）。
// 图的词汇表刻意很小：step 卡片 + actor 车道 + 一种箭头（runs after，`back` 只标回边）+
// 返回物标记。分析器的 kind / certainty / exact / region 不进载荷。
// 名字只在运行时成形（`` agent(`研究员${i + 1}`) ``）时静态能拿到的形状：第一个洞之前的
// 字面量（head）与最后一个洞之后的字面量（tail）。至少一个在场，两者都已 trim 且含实义字符。
// Bug 修复：这两个字段随 0a8b059f40 落进 contracts 与 v4 镜像，v3 这份漏改——.strict()
// 之下带插值名的工作流会让整个 display 验证失败、图整块消失，所以这里必须与 v4 逐字段对齐。
const escodeWorkflowNamePatternSchema = z
  .object({
    head: z.string().min(1).max(128).optional(),
    tail: z.string().min(1).max(128).optional(),
  })
  .strict();

// 一条边 = runs after；step 边与阶段边同形，`back` 只标循环回边。
const escodeWorkflowEdgeSchema = z
  .object({
    from: z.string().min(1).max(64),
    to: z.string().min(1).max(64),
    back: z.literal(true).optional(),
  })
  .strict();

const escodeCreateWorkflowCausalityGraphDisplaySchema = z
  .object({
    steps: z
      .array(
        z
          .object({
            id: z.string().min(1).max(64),
            kind: z.enum(["ask", "world-read"]),
            label: z.string().min(1).max(128),
            // 内联 `agent()` receiver 让 label 落到兜底串时，那个名字的静态形状。
            labelPattern: escodeWorkflowNamePatternSchema.optional(),
            line: z.number().int().positive().optional(),
            column: z.number().int().positive().optional(),
            lane: z.string().min(1).max(64),
            lanes: z.array(z.string().min(1).max(64)).max(32).optional(),
            // 展开自的站点 id，只出现在 may-set 车道展开的拷贝上（实时叠加的关联键）；
            // 加字段是 additive 的，不带它的旧载荷照常通过 .strict()。
            source: z.string().min(1).max(64).optional(),
            // 作者用 `phase("…")` 标记划入的阶段。
            // 与图的 phases / phaseEdges / exits 同进同退：全在场或全缺席。
            phase: z.string().min(1).max(64).optional(),
            repeat: z.enum(["stack", "serial"]).optional(),
          })
          .strict(),
      )
      .max(64),
    lanes: z
      .array(
        z
          .object({
            id: z.string().min(1).max(64),
            name: z.string().min(1).max(128).optional(),
            // `name` 缺席而 agent() 首参是带洞的模板串时的静态形状；与 name 互斥。
            namePattern: escodeWorkflowNamePatternSchema.optional(),
            line: z.number().int().positive().optional(),
            column: z.number().int().positive().optional(),
          })
          .strict(),
      )
      .max(32),
    // 参与者与交接；镜像 v4。
    participants: z
      .array(
        z
          .object({
            id: z.string().min(1).max(64),
            phase: z.string().min(1).max(64),
            lane: z.string().min(1).max(64),
            steps: z.array(z.string().min(1).max(64)).min(1).max(64),
            member: z
              .object({ index: z.number().int().nonnegative(), of: z.number().int().positive() })
              .strict()
              .optional(),
            many: z.literal(true).optional(),
          })
          .strict(),
      )
      .max(64),
    handoffs: z
      .array(
        escodeWorkflowEdgeSchema
          .extend({ types: z.array(z.string().min(1).max(128)).min(1).max(8).optional() })
          .strict(),
      )
      .max(256),
    // 阶段词汇表：作者施加的分组结构，主画面以它为节点。与 phaseEdges / exits / Step.phase
    // 全有或全无——零标记脚本全缺席，UI 据此退回 step/车道视图。零成员阶段也在表里。
    // `unphased` 无 name，显示名由 UI 本地化。
    phases: z
      .array(
        z
          .object({
            id: z.string().min(1).max(64),
            name: z.string().min(1).max(128).optional(),
            line: z.number().int().positive().optional(),
            column: z.number().int().positive().optional(),
            // 进入本阶段时还在跑的其他阶段（它们的 strand 尚未 join），阶段表序，不含自己，
            // 为空时缺席。是节点事实而不是边——控制没有从那里转移过来，所以不进 phaseEdges。
            // 时间轴据此把相邻阶段折成一条分叉的「带」，侧栏迷你轨道画成双线段。
            alongside: z.array(z.string().min(1).max(64)).min(1).max(32).optional(),
          })
          .strict(),
      )
      .max(32)
      .optional(),
    phaseEdges: z.array(escodeWorkflowEdgeSchema).max(128).optional(),
    // 控制流可在其后正常完成的阶段（阶段视图的「阶段 → 返回物」箭头）；组内可为空数组。
    exits: z.array(z.string().min(1).max(64)).max(32).optional(),
    sink: z.array(z.string().min(1).max(64)).max(64).optional(),
    truncated: z.boolean().optional(),
  })
  .strict();

export const escodeCreateWorkflowToolResultDisplaySchema = z
  .object({
    kind: z.literal("create_workflow"),
    ok: z.boolean(),
    errorCount: z.number().int().nonnegative(),
    diagnostics: z
      .array(
        z
          .object({
            line: z.number().int().nonnegative(),
            column: z.number().int().nonnegative(),
            code: z.number().int().nonnegative(),
            message: z.string().min(1).max(2_048),
          })
          .strict(),
      )
      .max(100),
    causalityGraph: escodeCreateWorkflowCausalityGraphDisplaySchema.optional(),
    truncated: z.boolean().optional(),
  })
  .strict();

const escodeToolResultObjectSchema = jsonObjectSchema.superRefine((result, context) => {
  const display = result.display;
  if (typeof display !== "object" || display === null || Array.isArray(display)) {
    return;
  }
  const kind = (display as Record<string, unknown>).kind;
  const schemaByKind: Record<string, z.ZodTypeAny> = {
    node_repl_images: escodeNodeReplImageToolResultDisplaySchema,
    create_workflow: escodeCreateWorkflowToolResultDisplaySchema,
    bash_output: bashOutputDisplaySchema,
  };
  const schema = typeof kind === "string" ? schemaByKind[kind] : undefined;
  if (!schema) return;
  const parsed = schema.safeParse(display);
  if (parsed.success) return;
  for (const issue of parsed.error.issues) {
    context.addIssue({ ...issue, path: ["display", ...issue.path] });
  }
});

export const escodeProtocolRequestIdSchema = z.union([z.string(), z.number().int()]);
export type ESCodeProtocolRequestId = z.infer<typeof escodeProtocolRequestIdSchema>;

export const escodeProtocolTraceSchema = z
  .object({
    traceparent: nonEmptyString.optional(),
    traceId: nonEmptyString.optional(),
    parentId: nonEmptyString.optional(),
    spanId: nonEmptyString.optional(),
  })
  .strict();
export type ESCodeProtocolTrace = z.infer<typeof escodeProtocolTraceSchema>;

export const escodeProtocolRequestSchema = z
  .object({
    id: escodeProtocolRequestIdSchema,
    method: nonEmptyString,
    params: z.unknown().optional(),
    trace: escodeProtocolTraceSchema.optional(),
  })
  .strict();
export type ESCodeProtocolRequest = z.infer<typeof escodeProtocolRequestSchema>;

export const escodeProtocolNotificationSchema = z
  .object({
    method: nonEmptyString,
    params: z.unknown().optional(),
    trace: escodeProtocolTraceSchema.optional(),
  })
  .strict();
export type ESCodeProtocolNotification = z.infer<typeof escodeProtocolNotificationSchema>;

export const escodeProtocolResponseSchema = z
  .object({
    id: escodeProtocolRequestIdSchema,
    result: z.unknown(),
  })
  .strict();
export type ESCodeProtocolResponse = z.infer<typeof escodeProtocolResponseSchema>;

export const escodeProtocolErrorSchema = z
  .object({
    id: escodeProtocolRequestIdSchema,
    error: z
      .object({
        code: z.number().int(),
        message: nonEmptyString,
        data: z.unknown().optional(),
      })
      .strict(),
  })
  .strict();
export type ESCodeProtocolError = z.infer<typeof escodeProtocolErrorSchema>;

export const escodeProtocolMessageSchema = z.union([
  escodeProtocolRequestSchema,
  escodeProtocolNotificationSchema,
  escodeProtocolResponseSchema,
  escodeProtocolErrorSchema,
]);
export type ESCodeProtocolMessage = z.infer<typeof escodeProtocolMessageSchema>;

export const escodeProtocolNotifications = {
  storageStartup: "startup/storageState",
  providerRuntimeHeadersCancelled: "interaction/providerRuntimeHeadersCancelled",
  mcpTelemetry: "process/mcpTelemetry",
  mcpResourceSamples: "process/mcpResourceSamples",
  toolExecResource: "process/toolExecResource",
  pluginOperationProgress: "plugins/operationProgress",
  processResourceSample: "process/resourceSample",
} as const;

/** 启动控制面独立于 task stream；数据库身份不可携带路径/凭据。 */
export const escodeStorageStartupStateSchema = z
  .object({
    schemaVersion: z.literal(1),
    attemptId: z.string().min(1).max(128),
    sequence: z.number().int().positive(),
    databaseId: z.string().min(1).max(128),
    databaseKind: z.enum(["session", "tasks-index"]),
    phase: z.enum(["checking", "waiting_for_lock", "migrating", "committing", "ready", "failed"]),
    // 包含锁内、版本 SQL 之前的可选 lastAppliedMigrationId；旧通知仍可解析。
    migration: databaseMigrationFactsSchema.optional(),
    elapsedMs: z.number().nonnegative().finite(),
    completed: z.number().int().nonnegative().optional(),
    total: z.number().int().nonnegative().optional(),
    errorCode: databaseStartupErrorCodeSchema.optional(),
    ...databaseStartupErrorDetailsSchema.shape,
  })
  .strict()
  .superRefine((state, context) => {
    if (state.phase === "failed" && !state.errorCode)
      context.addIssue({ code: "custom", message: "failed requires errorCode" });
  });
export type ESCodeStorageStartupState = z.infer<typeof escodeStorageStartupStateSchema>;

const escodeMcpTelemetryPlatformSchema = z.enum([
  "aix",
  "android",
  "darwin",
  "freebsd",
  "haiku",
  "linux",
  "netbsd",
  "openbsd",
  "sunos",
  "win32",
  "cygwin",
]);
const escodeMcpTelemetryArchSchema = z.enum([
  "arm",
  "arm64",
  "ia32",
  "loong64",
  "mips",
  "mipsel",
  "ppc",
  "ppc64",
  "riscv64",
  "s390",
  "s390x",
  "x64",
]);
const escodeMcpTelemetryBaseSchema = z
  .object({
    arch: escodeMcpTelemetryArchSchema,
    occurredAt: z.number().int().nonnegative(),
    platform: escodeMcpTelemetryPlatformSchema,
  })
  .strict();
const escodeMcpProcessTelemetryBaseShape = {
  mcpId: z
    .string()
    .regex(
      /^(?:builtin:(?:[A-Za-z0-9._~-]|%[0-9A-F]{2})+(?::(?:[A-Za-z0-9._~-]|%[0-9A-F]{2})+)*|(?:plugin|custom):[a-f0-9]{12})$/,
    ),
  mcpInstanceId: nonEmptyString,
  mcpIsolation: z.enum(["session", "workspace"]),
  mcpSource: z.enum(["builtin", "plugin", "custom"]),
} as const;

export const escodeMcpTelemetryEventSchema = z.discriminatedUnion("kind", [
  escodeMcpTelemetryBaseSchema
    .extend({
      kind: z.literal("process_start"),
      ...escodeMcpProcessTelemetryBaseShape,
    })
    .strict(),
  escodeMcpTelemetryBaseSchema
    .extend({
      kind: z.literal("process_crash"),
      ...escodeMcpProcessTelemetryBaseShape,
      affectedSessionCount: z.number().int().nonnegative().max(10_000),
      exitCode: z.number().int().nullable(),
      signal: nonEmptyString.nullable(),
      uptimeMs: z.number().finite().nonnegative().max(Number.MAX_SAFE_INTEGER),
    })
    .strict(),
  escodeMcpTelemetryBaseSchema
    .extend({
      kind: z.literal("session_startup"),
      configuredCount: z.number().int().nonnegative().max(10_000),
      connectedCount: z.number().int().nonnegative().max(10_000),
      failedCount: z.number().int().nonnegative().max(10_000),
      processCount: z.number().int().nonnegative().max(10_000),
      sessionId: nonEmptyString,
    })
    .strict(),
  escodeMcpTelemetryBaseSchema
    .extend({
      kind: z.literal("memory"),
      ...escodeMcpProcessTelemetryBaseShape,
      memoryKb: z.number().finite().nonnegative().max(Number.MAX_SAFE_INTEGER),
      memoryScope: z.enum(["process_tree", "direct_process"]),
      orphanSuspected: z.boolean(),
      ownerSessionCount: z.number().int().nonnegative().max(10_000),
      unownedSeconds: z.number().finite().nonnegative().max(Number.MAX_SAFE_INTEGER),
    })
    .strict(),
]);
export type ESCodeMcpTelemetryEvent = z.infer<typeof escodeMcpTelemetryEventSchema>;

/** MCP 每五分钟只探测一次，周期由生产者与设备总量过期判据共用。 */
export const ESCODE_MCP_RESOURCE_SAMPLE_INTERVAL_MS = 5 * 60_000;

export const escodeMcpResourceSampleSchema = z
  .object({
    mcpId: escodeMcpProcessTelemetryBaseShape.mcpId,
    instanceToken: z.string().regex(/^[A-Za-z0-9_-]{8,64}$/),
    sampledAt: z.number().int().nonnegative(),
    intervalMs: z.number().finite().positive(),
    processCount: z.number().int().positive().max(100_000),
    rssKbTotal: z.number().finite().nonnegative().max(Number.MAX_SAFE_INTEGER),
    rssKbMaxProcess: z.number().finite().nonnegative().max(Number.MAX_SAFE_INTEGER),
    cpuTimeMsDelta: z.number().finite().nonnegative().max(Number.MAX_SAFE_INTEGER),
    uptimeMinutes: z.number().int().nonnegative(),
    platform: escodeMcpTelemetryPlatformSchema,
    arch: escodeMcpTelemetryArchSchema,
    logicalCpuCount: z.number().int().positive().max(4_096),
    totalMemoryGb: z.number().int().nonnegative().max(1_048_576),
  })
  .strict();
export type ESCodeMcpResourceSample = z.infer<typeof escodeMcpResourceSampleSchema>;
// 通知输入有界；main 另按每个上报窗口的 32 个 MCP 分组执行事件额度。
export const escodeMcpResourceSamplesSchema = z.array(escodeMcpResourceSampleSchema).max(1_024);

export const BASH_RESOURCE_SAMPLE_INTERVAL_MS = 15_000;
export const BASH_RESOURCE_MAX_SAMPLES = 20;

/** Bash 子进程的有界完成事实；禁止命令、路径与会话标识进入遥测旁路。 */
export const escodeToolExecResourceSchema = z
  .object({
    // 同一完成事实可能经多个 Host 转发；随机标识仅供 main 去重，旧 CLI 缺字段仍兼容。
    completionToken: z.string().uuid().optional(),
    platform: escodeMcpTelemetryPlatformSchema,
    toolName: z.literal("bash"),
    durationMs: z.number().finite().min(BASH_RESOURCE_SAMPLE_INTERVAL_MS),
    exitKind: z.enum(["completed", "timeout", "killed", "error"]),
    treeRssKbPeak: z.number().finite().nonnegative().optional(),
    treeCpuTimeMs: z.number().finite().nonnegative().optional(),
    sampleCount: z.number().int().nonnegative().max(BASH_RESOURCE_MAX_SAMPLES),
    cliRssKb: z.number().finite().nonnegative(),
    systemFreeMemoryKb: z.number().finite().nonnegative(),
  })
  .strict();
export type ESCodeToolExecResource = z.infer<typeof escodeToolExecResourceSchema>;

export const escodeProcessResourceSampleSchema = z
  .object({
    platform: z.enum([
      "aix",
      "android",
      "darwin",
      "freebsd",
      "haiku",
      "linux",
      "netbsd",
      "openbsd",
      "sunos",
      "win32",
      "cygwin",
    ]),
    arch: z.enum([
      "arm",
      "arm64",
      "ia32",
      "loong64",
      "mips",
      "mipsel",
      "ppc",
      "ppc64",
      "riscv64",
      "s390",
      "s390x",
      "x64",
    ]),
    logicalCpuCount: z.number().int().positive().max(4_096),
    intervalMs: z
      .number()
      .int()
      .positive()
      .max(7 * 24 * 60 * 60 * 1_000),
    cpuCores: z.number().finite().nonnegative().max(4_096),
    cpuPercent: z.number().finite().nonnegative().max(100_000),
    rssKb: z.number().finite().nonnegative().max(Number.MAX_SAFE_INTEGER),
    /**
     * 以下四项为遥测新增字段，全部可选：旧 CLI 发来的样本仍能通过校验，因此
     * **不递增协议握手版本号**（握手版本是兼容性开关，不是字段版本）。
     */
    heapUsedKb: z.number().finite().nonnegative().max(Number.MAX_SAFE_INTEGER).optional(),
    uptimeMinutes: z
      .number()
      .int()
      .nonnegative()
      .max(10 * 365 * 24 * 60)
      .optional(),
    totalMemoryGb: z.number().int().nonnegative().max(1_048_576).optional(),
    /**
     * CLI 进程启动时随机生成的实例标识，仅供 app 侧 main 统计「同时存活几个 CLI 进程」
     * 与「最大单进程 RSS」。不进 ARMS 属性、不含 pid。收紧字符集是隐私红线的机械保障：
     * 路径、workspace 标识这类内容不可能通过校验。
     */
    instanceToken: z
      .string()
      .regex(/^[A-Za-z0-9_-]{8,64}$/)
      .optional(),
  })
  .strict();
export type ESCodeProcessResourceSample = z.infer<typeof escodeProcessResourceSampleSchema>;

export const escodeProcessChildProcessesParamsSchema = z.object({}).strict();
export const escodeProcessChildProcessSchema = z
  .object({
    pid: z.number().int().positive(),
    serverName: nonEmptyString,
    mcpSource: z.enum(["builtin", "plugin", "custom"]),
    /** 官方/第三方插件的插件名（`plugin:<name>:<key>` 的 name，或官方 host MCP 对应插件）；custom 无 */
    pluginName: nonEmptyString.optional(),
  })
  .strict();
export const escodeProcessChildProcessesResultSchema = z
  .object({
    processes: z.array(escodeProcessChildProcessSchema).max(10_000),
  })
  .strict();
export type ESCodeProcessChildProcess = z.infer<typeof escodeProcessChildProcessSchema>;
export type ESCodeProcessChildProcessesResult = z.infer<
  typeof escodeProcessChildProcessesResultSchema
>;

export type ESCodeDeliveryKind = z.infer<typeof escodeDeliveryKindSchema>;
// TurnStarted 与持久 message 必须共用同一来源词表；否则 live event 能通过而 cold
// message 在 app/agent 边界被拒绝，造成 continuous/replayable 语义分叉。
const escodeTurnInputSourceSchema = legacyEscodeSyntheticUserMessageSourceSchema;
export const escodeSessionPersistenceSchema = z.enum(["immediate", "deferred"]);
export type ESCodeSessionPersistence = z.infer<typeof escodeSessionPersistenceSchema>;
export type ESCodeWorkspaceRef = z.infer<typeof escodeWorkspaceRefSchema>;
export const escodePermissionOptionSchema = z
  .object({
    optionId: nonEmptyString,
    kind: nonEmptyString,
    name: nonEmptyString,
    description: z.string().optional(),
    response: escodePermissionResponseSchema,
  })
  .strict();

const escodeProtocolMcpEntrySchema = z
  .object({
    name: nonEmptyString,
    value: z.string(),
  })
  .strict();

const escodeProtocolMcpOAuthSchema = z.union([
  z
    .object({
      type: z.literal("client_credentials"),
      clientId: nonEmptyString,
      clientSecret: nonEmptyString,
      clientName: nonEmptyString.optional(),
      scope: z.string().optional(),
    })
    .strict(),
  z
    .object({
      type: z.literal("authorization_code"),
      clientId: nonEmptyString.optional(),
      clientSecret: nonEmptyString.optional(),
      clientName: nonEmptyString.optional(),
      redirectPath: nonEmptyString.optional(),
      scope: z.string().optional(),
    })
    .strict(),
]);

export const escodeProtocolMcpServerSchema = z.union([
  z
    .object({
      name: nonEmptyString,
      command: nonEmptyString,
      args: z.array(z.string()),
      env: z.array(escodeProtocolMcpEntrySchema),
      isolation: z.enum(["session", "workspace"]).optional(),
      protocolVersion: z.enum(["legacy", "auto", "2026-07-28"]).optional(),
      timeoutMs: z.number().int().positive().optional(),
    })
    .strict(),
  z
    .object({
      name: nonEmptyString,
      type: z.enum(["http", "sse"]),
      url: nonEmptyString,
      headers: z.array(escodeProtocolMcpEntrySchema),
      oauth: escodeProtocolMcpOAuthSchema.optional(),
      isolation: z.enum(["session", "workspace"]).optional(),
      protocolVersion: z.enum(["legacy", "auto", "2026-07-28"]).optional(),
      timeoutMs: z.number().int().positive().optional(),
    })
    .strict(),
]);
export type ESCodeProtocolMcpServer = z.infer<typeof escodeProtocolMcpServerSchema>;

export const escodeMcpServerStatusKindSchema = z.enum([
  "connecting",
  "connected",
  "disabled",
  "disconnected",
  "failed",
  "untrusted",
]);
export const MCP_SERVER_FAILURE_KINDS = [
  "config_invalid",
  "runtime_unavailable",
  "process_start_failed",
  "network_unreachable",
  "connection_timeout",
  "protocol_negotiation_failed",
  "tool_list_failed",
  "unexpected_disconnect",
  "oauth_authorization_failed",
  "official_origin_untrusted",
  "not_authenticated",
  "coding_plan_required",
  "server_not_found",
  "server_unavailable",
  "rate_limited",
  "server_internal_error",
  "protocol_error",
  "status_unavailable",
  "connection_failed",
] as const;
export const mcpServerFailureKindSchema = z.enum(MCP_SERVER_FAILURE_KINDS);
export type McpServerFailureKind = z.infer<typeof mcpServerFailureKindSchema>;
export const escodeMcpServerStatusSnapshotSchema = z
  .object({
    status: escodeMcpServerStatusKindSchema,
    transport: z.enum(["stdio", "http", "sse"]),
    toolCount: z.number().int().nonnegative(),
    updatedAt: nonEmptyString,
    error: z.string().optional(),
    failureKind: mcpServerFailureKindSchema.optional(),
    serverRequestId: nonEmptyString.optional(),
    protocolEra: z.enum(["legacy", "modern"]).optional(),
    authorization: z
      .object({
        type: z.literal("oauth_authorization_code"),
        authorizationUrl: nonEmptyString,
        startedAt: nonEmptyString,
      })
      .strict()
      .optional(),
  })
  .strict();
export type ESCodeMcpServerStatusSnapshot = z.infer<typeof escodeMcpServerStatusSnapshotSchema>;

export const escodeMcpListModeSchema = z.enum(["connect", "status"]);
export type ESCodeMcpListMode = z.infer<typeof escodeMcpListModeSchema>;

export const escodeMcpListParamsSchema = z
  .object({
    workspace: escodeWorkspaceRefSchema,
    mcpServers: z.array(escodeProtocolMcpServerSchema).optional(),
    mode: escodeMcpListModeSchema.default("connect"),
  })
  .strict();
export const escodeMcpListResultSchema = z
  .object({
    statuses: z.record(z.string(), escodeMcpServerStatusSnapshotSchema),
  })
  .strict();
export type ESCodeMcpListResult = z.infer<typeof escodeMcpListResultSchema>;

export const escodeSessionImportMessageSchema = z
  .object({
    role: z.enum(["user", "assistant"]),
    content: z.string(),
    timestamp: timestampMsSchema.optional(),
  })
  .strict();
export type ESCodeSessionImportMessage = z.infer<typeof escodeSessionImportMessageSchema>;

export const escodeSessionImportHistorySchema = z.discriminatedUnion("source", [
  z
    .object({
      source: z.literal("claudeCode"),
      title: z.string().optional(),
      createdAt: timestampMsSchema.optional(),
      updatedAt: timestampMsSchema.optional(),
      messages: z.array(escodeSessionImportMessageSchema).min(1),
    })
    .strict(),
  z
    .object({
      source: z.literal("sharedContext"),
      title: z.string().trim().min(1),
      createdAt: timestampMsSchema.optional(),
      markdown: z.string().min(1),
      provenance: z
        .object({
          shareId: z.string().trim().min(1),
          contextId: z.string().trim().min(1).optional(),
          shareUrl: z.string().url().optional(),
          status: z.enum(["pending", "reserved", "attached", "discarded"]).optional(),
          projectionSha256: z.string().regex(/^[0-9a-f]{64}$/u),
          artifactSetSha256: z.string().regex(/^[0-9a-f]{64}$/u),
          formatterVersion: z.literal(1),
          markdownSha256: z.string().regex(/^[0-9a-f]{64}$/u),
          installedArtifacts: z.array(
            z
              .object({
                artifactId: z.string().trim().min(1),
                workspaceRelativePath: z.string().trim().min(1),
              })
              .strict(),
          ),
        })
        .strict(),
    })
    .strict(),
]);
export type ESCodeSessionImportHistory = z.infer<typeof escodeSessionImportHistorySchema>;

export const escodeThoughtLevelOptionSchema = z
  .object({
    value: nonEmptyString,
    label: nonEmptyString,
    description: z.string().optional(),
  })
  .strict();
export const escodeModelReasoningOptionsSchema = z
  .object({
    levels: z.array(escodeThoughtLevelOptionSchema),
    defaultLevel: nonEmptyString.optional(),
  })
  .strict();
export type ESCodeModelReasoningOptions = z.infer<typeof escodeModelReasoningOptionsSchema>;

export const escodeModelFormatPropertiesSchema = completeModelPropertiesDataSchema.pick({
  inputFormat: true,
  outputFormat: true,
});
export type ESCodeModelFormatProperties = z.infer<typeof escodeModelFormatPropertiesSchema>;

export const escodeModelOptionSchema = z
  .object({
    ref: modelSelectionSchema,
    label: nonEmptyString,
    providerLabel: nonEmptyString.optional(),
    description: z.string().optional(),
    contextWindow: z.number().int().positive().optional(),
    maxOutputTokens: z.number().int().positive().optional(),
    reasoning: escodeModelReasoningOptionsSchema.optional(),
    properties: escodeModelFormatPropertiesSchema,
    disabledReason: z.string().optional(),
  })
  .strict();
export type ESCodeModelOption = z.infer<typeof escodeModelOptionSchema>;

export const escodeAccountAccessSchema = z.discriminatedUnion("planKind", [
  z
    .object({
      type: z.literal("zhipu-account"),
      family: z.enum(["zai", "bigmodel"]),
      planKind: z.literal("start-plan"),
    })
    .strict(),
  z
    .object({
      type: z.literal("zhipu-account"),
      family: z.enum(["zai", "bigmodel"]),
      planKind: z.literal("individual-coding-plan"),
    })
    .strict(),
  z
    .object({
      type: z.literal("zhipu-account"),
      family: z.enum(["zai", "bigmodel"]),
      planKind: z.literal("team-coding-plan"),
      productId: nonEmptyString,
      organizationId: nonEmptyString,
      projectId: nonEmptyString,
    })
    .strict(),
]);
export type ESCodeAccountAccess = z.infer<typeof escodeAccountAccessSchema>;

/** Active Model 固定的账号访问类别；当前商品和 Team scope 由账号服务在请求期解析。 */
export const escodeProviderAccountAccessSchema = z
  .object({
    type: z.literal("zhipu-account"),
    accountType: z.enum(["zai", "bigmodel"]),
    mode: z.enum(["start-plan", "individual-coding-plan", "team-coding-plan", "off-peak"]),
    entitled: z.boolean(),
  })
  .strict();
export type ESCodeProviderAccountAccess = z.infer<typeof escodeProviderAccountAccessSchema>;

export type ESCodeSessionMode = z.infer<typeof escodeSessionModeSchema>;
export type ESCodeSessionKind = z.infer<typeof escodeSessionKindSchema>;
export type ESCodeSessionGoal = z.infer<typeof escodeSessionGoalSchema>;

export const escodeSessionTodoItemSchema = z
  .object({
    content: nonEmptyString,
    status: z.enum(["pending", "in_progress", "completed"]),
    priority: z.enum(["high", "medium", "low"]),
  })
  .strict();
export const escodeSessionGoalStatsSchema = z
  .object({
    timeUsedSeconds: z.number().int().nonnegative(),
    tokensUsed: z.number().int().nonnegative(),
    tokenBudget: z.number().int().positive().nullable(),
    contextUsed: z.number().int().nonnegative(),
    contextWindow: z.number().int().nonnegative(),
    toolCallCount: z.number().int().nonnegative(),
    iterationCount: z.number().int().nonnegative(),
  })
  .strict();
export type ESCodeSessionGoalStats = z.infer<typeof escodeSessionGoalStatsSchema>;
export type ESCodeSessionGoalVerification = z.infer<typeof escodeSessionGoalVerificationSchema>;
export type ESCodeSessionGoalVerificationTimeline = z.infer<
  typeof escodeSessionGoalVerificationTimelineSchema
>;

export const escodeSessionTodoGroupSchema = z
  .object({
    id: nonEmptyString,
    source: z.enum(["goal_iteration", "session"]),
    goalIteration: z.number().int().positive().optional(),
    targetId: nonEmptyString.optional(),
    startedAt: timestampMsSchema.optional(),
    updatedAt: timestampMsSchema.optional(),
    todos: z.array(escodeSessionTodoItemSchema),
  })
  .strict();
export type ESCodeSessionTodoGroup = z.infer<typeof escodeSessionTodoGroupSchema>;

export const escodeSessionSettingsStateSchema = z
  .object({
    model: z
      .object({
        // 未绑定是合法恢复状态；不能为满足协议而伪造模型或阻断历史读取。
        current: modelSelectionSchema.optional(),
        available: z.array(escodeModelOptionSchema),
        lastUsed: modelSelectionSchema.optional(),
      })
      .strict(),
    thoughtLevel: z
      .object({
        enabled: z.boolean(),
        current: nonEmptyString.optional(),
        defaultLevel: nonEmptyString.optional(),
        available: z.array(escodeThoughtLevelOptionSchema),
      })
      .strict(),
    mode: z
      .object({
        current: escodeSessionModeSchema,
      })
      .strict(),
    permission: z
      .object({
        mode: escodeSessionModeSchema.optional(),
        rulesRevision: z.number().int().nonnegative().optional(),
      })
      .strict()
      .optional(),
  })
  .strict();
export type ESCodeSessionSettingsState = z.infer<typeof escodeSessionSettingsStateSchema>;
export const escodePendingPermissionSchema = z
  .object({
    requestId: nonEmptyString,
    toolCallId: nonEmptyString,
    toolName: nonEmptyString,
    reason: z.string(),
    riskLevel: z.enum(["low", "medium", "high", "critical"]),
    input: z.unknown().optional(),
    origin: escodeInteractionRequestOriginSchema.optional(),
    options: z.array(escodePermissionOptionSchema).min(1),
    requestedAt: timestampMsSchema,
  })
  .strict();
export type ESCodePendingPermission = z.infer<typeof escodePendingPermissionSchema>;

export const escodeActiveToolCallSchema = z
  .object({
    toolCallId: nonEmptyString,
    toolName: nonEmptyString,
    status: z.enum(["pending", "running", "completed", "failed", "denied"]),
    startedAt: timestampMsSchema.optional(),
  })
  .strict();
export type ESCodeActiveToolCall = z.infer<typeof escodeActiveToolCallSchema>;

export const escodeSessionProjectionSchema = z
  .object({
    sessionId: nonEmptyString,
    status: escodeSessionStatusSchema,
    mode: escodeSessionModeSchema,
    turnCount: z.number().int().nonnegative(),
    totalTokenCount: z.number().int().nonnegative(),
    contextUsed: z.number().int().nonnegative(),
    contextWindow: z.number().int().nonnegative(),
    currentTurnId: nonEmptyString.optional(),
    pendingPermissions: z.array(escodePendingPermissionSchema),
    activeToolCalls: z.array(escodeActiveToolCallSchema),
    backgroundJobs: z.array(jsonObjectSchema),
    target: escodeSessionGoalSchema.nullable().optional(),
    lastError: z
      .object({
        type: nonEmptyString,
        code: nonEmptyString.optional(),
        message: nonEmptyString,
        detail: z.string().optional(),
        attribution: errorAttributionSchema.optional(),
      })
      .strict()
      .optional(),
  })
  .strict();
export type ESCodeSessionProjection = z.infer<typeof escodeSessionProjectionSchema>;
export type ESCodeToolState = z.infer<typeof escodeToolStateSchema>;
export const escodeSlashCommandSchema = z
  .object({
    name: nonEmptyString,
    description: z.string(),
    inputHint: z.string().optional(),
    source: z.enum(["builtin", "custom"]).optional(),
  })
  .strict();
export type ESCodeSessionApiRetryStatus = z.infer<typeof escodeSessionApiRetryStatusSchema>;
export type ESCodeSessionContextUsage = z.infer<typeof escodeSessionContextUsageSchema>;
export const escodeModelStreamingKindSchema = z.enum([
  "start",
  "finish",
  "error",
  "text_start",
  "text_delta",
  "text_end",
  "reasoning_start",
  "reasoning_delta",
  "reasoning_end",
  "tool_input_start",
  "tool_input_delta",
  "tool_input_end",
  "tool_call",
]);
export const escodeModelStreamingEventPayloadSchema = z
  .object({
    assistantMessageId: z.string().optional(),
    delta: z.string().optional(),
    done: z.boolean().optional(),
    input: z.unknown().optional(),
    kind: escodeModelStreamingKindSchema,
    partId: z.string().optional(),
    providerExecuted: z.boolean().optional(),
    toolCallId: z.string().optional(),
    toolName: z.string().optional(),
  })
  .strict();
export const escodeSessionStateSnapshotSchema = z
  .object({
    protocol: z
      .object({
        name: z.literal(ESCODE_PROTOCOL_NAME),
        version: z.literal(ESCODE_PROTOCOL_VERSION),
      })
      .strict(),
    session: escodeSessionInfoSchema,
    settings: escodeSessionSettingsStateSchema,
    projection: escodeSessionProjectionSchema,
    runtime: escodeSessionRuntimeStateSchema,
    messages: z.array(escodeMessageWithPartsSchema),
    goalStats: escodeSessionGoalStatsSchema.optional(),
    todos: z.array(escodeSessionTodoItemSchema).optional(),
    todoGroups: z.array(escodeSessionTodoGroupSchema).optional(),
    slashCommands: z.array(escodeSlashCommandSchema).optional(),
  })
  .strict();
export type ESCodeSessionStateSnapshot = z.infer<typeof escodeSessionStateSnapshotSchema>;

export const escodeEventEnvelopeSchema = z
  .object({
    eventId: nonEmptyString,
    sessionId: nonEmptyString,
    turnId: nonEmptyString.optional(),
    seq: z.number().int().nonnegative(),
    traceId: nonEmptyString.optional(),
    timestamp: timestampMsSchema,
    deliveryKind: escodeDeliveryKindSchema.optional(),
  })
  .strict();

const escodeComputerUseOperationEventBaseSchema = z
  .object({
    eventId: nonEmptyString,
    sequenceNumber: z.number().int().nonnegative(),
    sessionId: nonEmptyString,
    timestamp: timestampMsSchema,
  })
  .strict();

const escodeComputerUseTurnStartedEventSchema = escodeComputerUseOperationEventBaseSchema.extend({
  kind: z.literal("turn-started"),
  turnId: nonEmptyString,
});
const escodeComputerUseTurnCompletedEventSchema = escodeComputerUseOperationEventBaseSchema.extend({
  kind: z.literal("turn-completed"),
  turnId: nonEmptyString,
});
const escodeComputerUseTurnFailedEventSchema = escodeComputerUseOperationEventBaseSchema.extend({
  kind: z.literal("turn-failed"),
  turnId: nonEmptyString,
});
const escodeComputerUseToolScheduledEventSchema = escodeComputerUseOperationEventBaseSchema.extend({
  kind: z.literal("tool-scheduled"),
  turnId: nonEmptyString,
  toolCallId: nonEmptyString,
  toolName: nonEmptyString,
  // 这个 cell 是否在用 Computer Use。只表达布尔事实，不再携带动作名——旧的
  // operationAction 靠从模型源码里抽取动作名得到，SDK 面一变就整体失配（见
  // bootstrap/src/escode-protocol/computer-use-operation-event.ts 的 usesComputerUse）。
  // 只挂在 scheduled 上：ToolCallStartedPayload 没有 input，start 时已拿不到模型源码。
  computerUse: z.literal(true).optional(),
});
const escodeComputerUseToolStartedEventSchema = escodeComputerUseOperationEventBaseSchema.extend({
  kind: z.literal("tool-started"),
  turnId: nonEmptyString.optional(),
  toolCallId: nonEmptyString,
  toolName: nonEmptyString.optional(),
});
const escodeComputerUseSessionClosedEventSchema = escodeComputerUseOperationEventBaseSchema.extend({
  kind: z.literal("session-closed"),
});

export const escodeComputerUseOperationEventSchema = z.discriminatedUnion("kind", [
  escodeComputerUseTurnStartedEventSchema,
  escodeComputerUseTurnCompletedEventSchema,
  escodeComputerUseTurnFailedEventSchema,
  escodeComputerUseToolScheduledEventSchema,
  escodeComputerUseToolStartedEventSchema,
  escodeComputerUseSessionClosedEventSchema,
]);
export type ESCodeComputerUseOperationEvent = z.infer<typeof escodeComputerUseOperationEventSchema>;

export const escodeSessionEventTypeSchema = z.enum([
  "session.created",
  "session.resumed",
  "session.updated",
  "session.titleUpdated",
  "session.closed",
  "turn.started",
  "turn.steerQueued",
  "turn.steerDrained",
  "turn.completed",
  "turn.failed",
  "message.upserted",
  "message.removed",
  "part.started",
  "part.delta",
  "part.upserted",
  "part.removed",
  "model.streaming",
  "tool.updated",
  "permission.requested",
  "permission.resolved",
  "userInput.requested",
  "userInput.resolved",
  "checkpoint.created",
  "rewind.triggered",
  "streamRecovery.updated",
]);
export type ESCodeSessionEventType = z.infer<typeof escodeSessionEventTypeSchema>;

export const escodeProtocolErrorDetailSchema = z
  .object({
    type: nonEmptyString,
    message: nonEmptyString,
    stack: z.string().optional(),
    code: z.string().optional(),
    detail: z.string().optional(),
    underlyingErrorMessage: z.string().optional(),
    underlyingErrorDetail: z.string().optional(),
    attribution: errorAttributionSchema.optional(),
    retryable: z.boolean().optional(),
    data: z.unknown().optional(),
  })
  .strict();
export const escodeSessionCreatedEventPayloadSchema = z
  .object({
    mode: escodeSessionModeSchema,
    contextWindow: z.number().int().nonnegative(),
  })
  .strict();
export const escodeSessionResumedEventPayloadSchema = z
  .object({
    directory: nonEmptyString,
    interruptedToolCount: z.number().int().nonnegative(),
    messageCount: z.number().int().nonnegative(),
    partCount: z.number().int().nonnegative(),
    recoveredCompactTimelineCount: z.number().int().nonnegative().optional(),
    recoveredSteerInputCount: z.number().int().nonnegative().optional(),
    resumedTodoCount: z.number().int().nonnegative().optional(),
  })
  .strict();
export const escodeSessionTitleUpdatedEventPayloadSchema = z
  .object({
    messageID: nonEmptyString.optional(),
    previousTitle: z.string(),
    source: z.enum(["default", "first_input", "generated", "custom"]),
    title: z.string(),
  })
  .strict();
export const escodeTurnStartedEventPayloadSchema = z
  .object({
    turnNumber: z.number().int().nonnegative(),
    input: z.string(),
    inputId: nonEmptyString.optional(),
    queryId: nonEmptyString.optional(),
    inputSource: escodeTurnInputSourceSchema.optional(),
    inputVisibility: escodeMessageVisibilitySchema.optional(),
    executionKind: z.enum(["agent", "controlOnly"]).optional(),
    targetId: nonEmptyString.optional(),
    messageId: nonEmptyString.optional(),
    foregroundExecutionId: nonEmptyString.optional(),
    intent: jsonObjectSchema.optional(),
    originMeta: jsonObjectSchema.optional(),
    // runtime 会透传后台唤醒来源，strict schema 必须同步声明以免丢弃整条事件。
    backgroundSource: z.enum(["bash", "subagent"]).optional(),
    attachments: z.array(jsonObjectSchema).optional(),
  })
  .strict();
const escodeTurnSteerSourceSchema = z.enum(["plan_approval_feedback", "workflow_refine_feedback"]);
const escodeTurnSteerCommandKindSchema = z.enum(["sendText", "sendGoalCommand", "compact"]);
const escodeTurnSteerDeliverySchema = z.enum(["queue", "guide"]);

export const escodeTurnSteerQueuedEventPayloadSchema = z
  .object({
    pendingInputId: nonEmptyString,
    inputId: nonEmptyString.optional(),
    queryId: nonEmptyString.optional(),
    input: z.string(),
    inputPreview: z.string(),
    inputSize: z.number().int().nonnegative(),
    commandKind: escodeTurnSteerCommandKindSchema.optional(),
    source: escodeTurnSteerSourceSchema.optional(),
    toolDisallowlist: z.array(nonEmptyString).optional(),
    delivery: escodeTurnSteerDeliverySchema.optional(),
    targetTurnId: nonEmptyString,
    queueLength: z.number().int().nonnegative(),
    intent: jsonObjectSchema.optional(),
  })
  .strict();
export const escodeTurnSteerDrainedEventPayloadSchema = z
  .object({
    pendingInputIds: z.array(nonEmptyString),
    queryIds: z.array(nonEmptyString).optional(),
    targetTurnId: nonEmptyString,
    injectedMessageIds: z.array(nonEmptyString),
    drainedInputs: z
      .array(
        z
          .object({
            pendingInputId: nonEmptyString,
            messageId: nonEmptyString,
            text: z.string(),
            delivery: escodeTurnSteerDeliverySchema.optional(),
            intent: jsonObjectSchema.optional(),
            toolDisallowlist: z.array(nonEmptyString).optional(),
          })
          .strict(),
      )
      .optional(),
  })
  .strict();
export const escodeTurnCompletedEventPayloadSchema = z
  .object({
    response: z.string(),
    tokenCount: z.number().int().nonnegative(),
    usage: z.unknown().optional(),
    toolCallCount: z.number().int().nonnegative(),
    historyRoundCount: z.number().int().nonnegative().optional(),
    duration: z.number().nonnegative(),
    // runtime turn.completed 会附带 cacheStats，协议 schema 之前漏掉该字段。
    // strict 校验失败会让桌面端丢掉终态事件，表现为消息已完成但 UI 一直没有回复。
    cacheStats: z
      .object({
        totalMessages: z.number().int().nonnegative(),
        cachedMessages: z.number().int().nonnegative(),
        lastCacheHit: z.boolean(),
        cacheReadTokens: z.number().int().nonnegative().optional(),
      })
      .strict()
      .optional(),
    inputId: nonEmptyString.optional(),
    resultType: z.enum([
      "success",
      // "cancelled": 用户主动中断属于正常结束，复用 turn.completed 上报，避免被映射成 turn.failed。
      "cancelled",
      "error_max_turns",
      "error_max_budget",
      "error_during_execution",
      "error_max_tool_calls",
    ]),
    backgroundSubagentResultConsumed: z.boolean().optional(),
  })
  .strict();
export const escodeTurnFailedEventPayloadSchema = z
  .object({
    error: escodeProtocolErrorDetailSchema,
    turnPhase: z.string(),
    inputId: nonEmptyString.optional(),
    backgroundSubagentResultConsumed: z.boolean().optional(),
  })
  .strict();
export const escodeMessageUpsertedEventPayloadSchema = z
  .object({
    content: z.string(),
    attachments: z.array(z.unknown()).optional(),
    toolCalls: z.array(z.unknown()).optional(),
    type: z.string().optional(),
    compactBoundary: z.unknown().optional(),
  })
  .strict();
export const escodeMessageRemovedEventPayloadSchema = z
  .object({
    messageId: nonEmptyString,
    reason: z.string().optional(),
  })
  .strict();
export const escodeMessagePartDeltaEventPayloadSchema = z
  .object({
    messageId: nonEmptyString,
    partId: nonEmptyString,
    field: z.enum(["text", "reasoning", "input", "output"]).optional(),
    delta: z.string(),
  })
  .strict();
export const escodeMessagePartUpsertedEventPayloadSchema = z
  .object({
    part: escodeMessagePartSchema,
  })
  .strict();
export const escodeMessagePartRemovedEventPayloadSchema = z
  .object({
    messageId: nonEmptyString,
    partId: nonEmptyString,
    reason: z.string().optional(),
  })
  .strict();
const escodeToolCallBasePayloadSchema = z
  .object({
    toolCallId: nonEmptyString,
    toolName: z.string().optional(),
    parentToolCallId: nonEmptyString.optional(),
    source: z.enum(["subagent"]).optional(),
    agentId: nonEmptyString.optional(),
    agentType: nonEmptyString.optional(),
    // subagent mirror 会携带后台归因；strict schema 漏字段会让 session/event 整条被丢弃。
    background: z.boolean().optional(),
    childSessionId: nonEmptyString.optional(),
    childToolCallId: nonEmptyString.optional(),
    description: z.string().optional(),
  })
  .strict();

export const escodeToolUpdatedEventPayloadSchema = z.discriminatedUnion("kind", [
  escodeToolCallBasePayloadSchema
    .extend({
      kind: z.literal("scheduled"),
      // 修复：CLI 调度事件已携带所属消息 ID；漏声明会让严格校验丢弃整条事件。
      assistantMessageId: nonEmptyString.optional(),
      toolName: nonEmptyString,
      input: z.unknown().optional(),
      inputByteLength: z.number().int().nonnegative().optional(),
      inputOmitted: z.boolean().optional(),
      inputRef: z.literal("model_stream").optional(),
      dependencies: z.array(nonEmptyString).optional(),
      parallelGroupIndex: z.number().int().nonnegative().optional(),
      canRunParallel: z.boolean().optional(),
      schedule: jsonObjectSchema.optional(),
    })
    .strict(),
  escodeToolCallBasePayloadSchema
    .extend({
      kind: z.literal("started"),
      startedAt: protocolInstantSchema,
    })
    .strict(),
  escodeToolCallBasePayloadSchema
    .extend({
      kind: z.literal("progress"),
      elapsedMs: z.number().nonnegative().optional(),
      pid: z.number().int().optional(),
      stdoutBytes: z.number().int().nonnegative().optional(),
      stderrBytes: z.number().int().nonnegative().optional(),
      outputBytes: z.number().int().nonnegative().optional(),
      outputPreview: executionOutputPreviewSchema.optional(),
      stdoutTail: z.string().optional(),
      stderrTail: z.string().optional(),
    })
    .strict(),
  escodeToolCallBasePayloadSchema
    .extend({
      kind: z.literal("result"),
      result: escodeToolResultObjectSchema,
      duration: z.number().nonnegative(),
    })
    .strict(),
  escodeToolCallBasePayloadSchema
    .extend({
      kind: z.literal("error"),
      error: escodeProtocolErrorDetailSchema,
    })
    .strict(),
  z
    .object({
      kind: z.literal("batch"),
      toolCallIds: z.array(nonEmptyString),
      successCount: z.number().int().nonnegative(),
      errorCount: z.number().int().nonnegative(),
    })
    .strict(),
  escodeToolCallBasePayloadSchema
    .extend({
      kind: z.literal("raw"),
      payload: jsonObjectSchema,
    })
    .strict(),
]);
export const escodePermissionRequestedEventPayloadSchema = z
  .object({
    requestId: nonEmptyString.optional(),
    toolCallId: nonEmptyString,
    toolName: nonEmptyString,
    riskLevel: z.enum(["low", "medium", "high", "critical"]),
    reason: z.string(),
    input: z.unknown(),
    suggestedPermissionUpdates: z.array(escodePermissionUpdateSchema).optional(),
    origin: escodeInteractionRequestOriginSchema.optional(),
    options: z.array(escodePermissionOptionSchema).min(1),
    childSessionId: nonEmptyString.optional(),
    background: z.boolean().optional(),
  })
  .strict();
export const escodePermissionResolvedEventPayloadSchema = z
  .object({
    requestId: nonEmptyString.optional(),
    toolCallId: nonEmptyString,
    toolName: nonEmptyString.optional(),
    decision: escodePermissionDecisionSchema.optional(),
    reason: z.string().optional(),
    modifiedInput: z.unknown().optional(),
    inputSummary: z.unknown().optional(),
    childSessionId: nonEmptyString.optional(),
    background: z.boolean().optional(),
  })
  .strict();
export const escodeUserInputRequestedEventPayloadSchema = z
  .object({
    requestId: nonEmptyString,
    prompt: z.string(),
    inputType: z.enum(["text", "choice", "confirm"]).optional(),
    choices: z.array(z.string()).optional(),
  })
  .strict();
export const escodeUserInputResolvedEventPayloadSchema = z
  .object({
    requestId: nonEmptyString,
    value: z.unknown().optional(),
    cancelled: z.boolean().optional(),
  })
  .strict();
export const escodeSessionClosedEventPayloadSchema = z
  .object({
    reason: z.string().optional(),
  })
  .strict();

function escodeSessionEventEnvelopeFor<T extends ESCodeSessionEventType>(
  type: T,
  payload: z.ZodTypeAny,
) {
  return escodeEventEnvelopeSchema.extend({
    type: z.literal(type),
    payload: payload.optional(),
  });
}

export const escodeSessionEventSchema = z.discriminatedUnion("type", [
  escodeSessionEventEnvelopeFor("session.created", escodeSessionCreatedEventPayloadSchema),
  escodeSessionEventEnvelopeFor("session.resumed", escodeSessionResumedEventPayloadSchema),
  escodeSessionEventEnvelopeFor("session.updated", jsonObjectSchema),
  escodeSessionEventEnvelopeFor("session.titleUpdated", escodeSessionTitleUpdatedEventPayloadSchema),
  escodeSessionEventEnvelopeFor("session.closed", escodeSessionClosedEventPayloadSchema),
  escodeSessionEventEnvelopeFor("turn.started", escodeTurnStartedEventPayloadSchema),
  escodeSessionEventEnvelopeFor("turn.steerQueued", escodeTurnSteerQueuedEventPayloadSchema),
  escodeSessionEventEnvelopeFor("turn.steerDrained", escodeTurnSteerDrainedEventPayloadSchema),
  escodeSessionEventEnvelopeFor("turn.completed", escodeTurnCompletedEventPayloadSchema),
  escodeSessionEventEnvelopeFor("turn.failed", escodeTurnFailedEventPayloadSchema),
  escodeSessionEventEnvelopeFor("message.upserted", escodeMessageUpsertedEventPayloadSchema),
  escodeSessionEventEnvelopeFor("message.removed", escodeMessageRemovedEventPayloadSchema),
  escodeSessionEventEnvelopeFor("part.started", escodeMessagePartUpsertedEventPayloadSchema),
  escodeSessionEventEnvelopeFor("part.delta", escodeMessagePartDeltaEventPayloadSchema),
  escodeSessionEventEnvelopeFor("part.upserted", escodeMessagePartUpsertedEventPayloadSchema),
  escodeSessionEventEnvelopeFor("part.removed", escodeMessagePartRemovedEventPayloadSchema),
  escodeSessionEventEnvelopeFor("model.streaming", escodeModelStreamingEventPayloadSchema),
  escodeSessionEventEnvelopeFor("tool.updated", escodeToolUpdatedEventPayloadSchema),
  escodeSessionEventEnvelopeFor("permission.requested", escodePermissionRequestedEventPayloadSchema),
  escodeSessionEventEnvelopeFor("permission.resolved", escodePermissionResolvedEventPayloadSchema),
  escodeSessionEventEnvelopeFor("userInput.requested", escodeUserInputRequestedEventPayloadSchema),
  escodeSessionEventEnvelopeFor("userInput.resolved", escodeUserInputResolvedEventPayloadSchema),
  escodeSessionEventEnvelopeFor("checkpoint.created", jsonObjectSchema),
  escodeSessionEventEnvelopeFor("rewind.triggered", jsonObjectSchema),
  escodeSessionEventEnvelopeFor("streamRecovery.updated", jsonObjectSchema),
]);
export type ESCodeSessionEvent = z.infer<typeof escodeSessionEventSchema>;

export const escodeSessionEventsResultSchema = z
  .object({
    events: z.array(escodeSessionEventSchema),
  })
  .strict();
export const escodeSessionMessagesResultSchema = z
  .object({
    messages: z.array(escodeMessageWithPartsSchema),
  })
  .strict();
export const escodeStateUpdatedNotificationSchema = z
  .object({
    type: z.literal("state.updated"),
    scope: z.enum(["server", "workspace", "session"]),
    workspace: escodeWorkspaceRefSchema.optional(),
    sessionId: nonEmptyString.optional(),
    revision: z.number().int().nonnegative(),
    reason: z.string().optional(),
    patch: z.unknown(),
  })
  .strict();
export type ESCodeStateUpdatedNotification = z.infer<typeof escodeStateUpdatedNotificationSchema>;

export const escodeSessionSubscribeParamsSchema = z
  .object({
    sessionId: nonEmptyString,
    deliveryKind: escodeDeliveryKindSchema,
    afterSeq: z.number().int().nonnegative().optional(),
    includeSnapshot: z.boolean().default(false),
  })
  .strict();
export type ESCodeSessionSubscribeParams = z.infer<typeof escodeSessionSubscribeParamsSchema>;

export const escodeSessionSubscribeResultSchema = z
  .object({
    sessionId: nonEmptyString,
    eventSeq: z.number().int().nonnegative(),
    events: z.array(escodeSessionEventSchema),
    snapshot: escodeSessionStateSnapshotSchema.optional(),
  })
  .strict();
export const escodeSessionListResultSchema = z
  .object({
    sessions: z.array(escodeSessionInfoSchema),
  })
  .strict();

const escodeSessionSubagentBaseSchema = z
  .object({
    childSessionId: nonEmptyString,
    agentId: nonEmptyString.optional(),
    toolCallId: nonEmptyString.optional(),
    subagentType: nonEmptyString,
    title: nonEmptyString,
    summary: z.string().optional(),
    startedAt: z.number().int().nonnegative().optional(),
    endedAt: z.number().int().nonnegative().optional(),
  })
  .strict();

export const escodeSessionRunningSubagentSchema = escodeSessionSubagentBaseSchema.extend({
  status: z.enum(["running", "waiting", "blocked"]),
});
export type ESCodeSessionRunningSubagent = z.infer<typeof escodeSessionRunningSubagentSchema>;

export const escodeSessionEndedSubagentSchema = escodeSessionSubagentBaseSchema.extend({
  status: z.enum(["success", "failed", "cancelled", "lost"]),
});
export type ESCodeSessionEndedSubagent = z.infer<typeof escodeSessionEndedSubagentSchema>;

export const escodeSessionSubagentsResultSchema = z
  .object({
    revision: z.number().int().nonnegative(),
    childSessionIds: z.array(nonEmptyString),
    running: z.array(escodeSessionRunningSubagentSchema),
    ended: z
      .object({
        total: z.number().int().nonnegative(),
        items: z.array(escodeSessionEndedSubagentSchema),
        nextCursor: nonEmptyString.optional(),
      })
      .strict(),
  })
  .strict();
export type ESCodeSessionSubagentsResult = z.infer<typeof escodeSessionSubagentsResultSchema>;
export const escodeSessionCreateParamsSchema = z
  .object({
    sessionId: nonEmptyString.optional(),
    workspace: escodeWorkspaceRefSchema,
    parentSessionId: nonEmptyString.optional(),
    mode: escodeSessionModeSchema.optional(),
    model: modelSelectionSchema.optional(),
    persistence: escodeSessionPersistenceSchema.optional(),
    thoughtLevel: nonEmptyString.optional(),
    titleGenerationEnabled: z.boolean().optional(),
    mcpServers: z.array(escodeProtocolMcpServerSchema).optional(),
    toolAllowlist: z.array(nonEmptyString).optional(),
    toolDenylist: z.array(nonEmptyString).optional(),
    importedHistory: escodeSessionImportHistorySchema.optional(),
    // host 只按本地服务装配/远程/端形态决定是否注册工具，不读取灰度；
    // 缺省不下发 = 不注册；灰度与套餐准入在实际创建的 Host handler 校验。
    offPeakToolEnabled: z.boolean().optional(),
    // 动态工作流灰度：与 offPeakToolEnabled 同一
    // 模式——host 裁决后下发，缺省不下发 = 不注册工作流工具簇（fail-closed）。
    dynamicWorkflowEnabled: z.boolean().optional(),
  })
  .strict();
export type ESCodeSessionCreateParams = z.infer<typeof escodeSessionCreateParamsSchema>;

export const escodeSessionResumeParamsSchema = z
  .object({
    sessionId: nonEmptyString,
    workspace: escodeWorkspaceRefSchema.optional(),
    // 旧 session 尚无 runtime/model_selection entry 时，由同 task 的索引元数据提供迁移 hint。
    thoughtLevel: nonEmptyString.optional(),
    mcpServers: z.array(escodeProtocolMcpServerSchema).optional(),
    // 冷恢复重建 runtime 时必须沿用 create 的工具面约束（否则会绕过 allow/deny，尤其 CUA 会话）。
    toolAllowlist: z.array(nonEmptyString).optional(),
    toolDenylist: z.array(nonEmptyString).optional(),
    // 与 create 同语义；resume 不带会导致冷恢复丢 Off-Peak 工具面。
    offPeakToolEnabled: z.boolean().optional(),
    // 与 create 同语义；resume 不带会导致冷恢复丢工作流工具簇。
    dynamicWorkflowEnabled: z.boolean().optional(),
  })
  .strict();
export type ESCodeSessionResumeParams = z.infer<typeof escodeSessionResumeParamsSchema>;

export const escodeSessionListParamsSchema = z
  .object({
    workspace: escodeWorkspaceRefSchema.optional(),
    // 显式身份查询包含隐藏会话；普通列表仍只返回主任务，避免索引修复激活 runtime。
    sessionIds: z.array(nonEmptyString).min(1).max(64).optional(),
    includeArchived: z.boolean().default(false),
    limit: z.number().int().positive().optional(),
  })
  .strict();
export type ESCodeSessionListParams = z.infer<typeof escodeSessionListParamsSchema>;

export const escodeSessionSubagentsParamsSchema = z
  .object({
    sessionId: nonEmptyString,
    endedCursor: nonEmptyString.optional(),
    endedLimit: z.number().int().positive().max(100).default(20),
  })
  .strict();
export type ESCodeSessionSubagentsParams = z.infer<typeof escodeSessionSubagentsParamsSchema>;

export const escodeUsageStatsParamsSchema = z
  .object({
    range: z.enum(APP_USAGE_RANGES),
    timeZone: z.string().optional(),
  })
  .strict();
export const escodeUsageStatsResultSchema = appUsageSnapshotSchema;
export const escodeTaskTokenUsageParamsSchema = z
  .object({
    sessionId: nonEmptyString,
  })
  .strict();
export const escodeTaskTokenUsageResultSchema = z
  .object({
    sessionId: nonEmptyString,
    totalTokens: z.number().int().nonnegative(),
    inputTokens: z.number().int().nonnegative(),
    outputTokens: z.number().int().nonnegative(),
    reasoningTokens: z.number().int().nonnegative(),
    cacheCreationTokens: z.number().int().nonnegative(),
    cacheReadTokens: z.number().int().nonnegative(),
    modelRequestCount: z.number().int().nonnegative(),
    modelErrorCount: z.number().int().nonnegative(),
    inputBaselineBySource: z.record(z.string(), z.number().int().nonnegative()),
  })
  .strict();
export type ESCodeTaskTokenUsageResult = z.infer<typeof escodeTaskTokenUsageResultSchema>;

export const escodeSessionReadParamsSchema = z
  .object({
    sessionId: nonEmptyString,
    deliveryKind: escodeDeliveryKindSchema.optional(),
    messageLimit: z.number().int().positive().optional(),
    afterSeq: z.number().int().nonnegative().optional(),
  })
  .strict();
export type ESCodeSessionReadParams = z.infer<typeof escodeSessionReadParamsSchema>;

export const escodeSessionMessagesParamsSchema = z
  .object({
    sessionId: nonEmptyString,
    afterMessageId: nonEmptyString.optional(),
    limit: z.number().int().positive().optional(),
  })
  .strict();
export type ESCodeSessionMessagesParams = z.infer<typeof escodeSessionMessagesParamsSchema>;

export const escodeSessionEventsParamsSchema = z
  .object({
    sessionId: nonEmptyString,
    afterSeq: z.number().int().nonnegative().optional(),
    limit: z.number().int().positive().optional(),
  })
  .strict();
export type ESCodeSessionEventsParams = z.infer<typeof escodeSessionEventsParamsSchema>;

export const escodeSessionRuntimePreferencesScopeSchema = z.enum([
  "runtime-materialization",
  "user-execution",
]);
export type ESCodeSessionRuntimePreferencesScope = z.infer<
  typeof escodeSessionRuntimePreferencesScopeSchema
>;

export const ESCODE_SESSION_RUNTIME_PREFERENCES_REQUEST_TIMEOUT_MS = 15_000;

export const escodeSessionRequestRuntimePreferencesParamsSchema = z
  .object({
    sessionId: nonEmptyString,
    scope: escodeSessionRuntimePreferencesScopeSchema,
  })
  .strict();
export type ESCodeSessionRequestRuntimePreferencesParams = z.infer<
  typeof escodeSessionRequestRuntimePreferencesParamsSchema
>;

export const DEFAULT_ESCODE_MODEL_CONTEXT_BUDGET_STRATEGY = "preflight-v1" as const;

// 3.12.2：legacy 仅为旧协议接收兼容；Runtime 一律归一为上面的共享默认策略。
export const escodeModelContextBudgetStrategySchema = z.enum(["legacy", "preflight-v1"]);
export type ESCodeModelContextBudgetStrategy = z.infer<typeof escodeModelContextBudgetStrategySchema>;

export const escodeSessionRuntimePreferencesResultSchema = z
  .object({
    nativeSearchEnhancementsEnabled: z.boolean(),
    memoryEnabled: z.boolean().default(false),
    askUserQuestionAutoResolutionEnabled: z.boolean().default(true),
    integratedTerminalShell: integratedTerminalShellSelectionSchema.optional(),
    // 兼容旧 Host：缺少字段时在协议解析边界使用当前默认策略。
    modelContextBudgetStrategy: escodeModelContextBudgetStrategySchema.default(
      DEFAULT_ESCODE_MODEL_CONTEXT_BUDGET_STRATEGY,
    ),
  })
  .strict();
export type ESCodeSessionRuntimePreferencesResult = z.infer<
  typeof escodeSessionRuntimePreferencesResultSchema
>;

/**
 * App 在提交 prompt 前只读采集的 IAB 可见状态。该字段只用于 provider-visible
 * ambient context，不进入用户可见 transcript；内容有界，禁止携带页面正文或凭据。
 */
export const escodeBrowserAmbientContextSchema = z
  .object({
    tabCount: z.number().int().positive().max(100),
    currentUrl: z.string().trim().min(1).max(4096).optional(),
  })
  .strict();
export type ESCodeBrowserAmbientContext = z.infer<typeof escodeBrowserAmbientContextSchema>;

export const escodeSessionSendParamsSchema = z
  .object({
    sessionId: nonEmptyString,
    modelSelection: modelSelectionSchema.optional(),
    modelExecution: modelExecutionSchema.optional(),
    inputId: nonEmptyString.optional(),
    queryId: nonEmptyString.optional(),
    content: z.string(),
    attachments: z.array(jsonObjectSchema).optional(),
    browserAmbientContext: escodeBrowserAmbientContextSchema.optional(),
    expectedRevision: z.number().int().nonnegative().optional(),
    expectedProviderRevision: nonEmptyString.optional(),
    automationId: nonEmptyString.optional(),
    offPeakTaskId: nonEmptyString.optional(),
    offPeakRunType: z.enum(["init", "resume"]).optional(),
    botDeliveryTarget: escodeAutomationBotDeliveryTargetSchema.optional(),
    toolDenylist: z.array(nonEmptyString).optional(),
  })
  .strict()
  .superRefine((payload, context) => {
    if (payload.automationId && payload.offPeakTaskId) {
      context.addIssue({
        code: z.ZodIssueCode.custom,
        message: "automationId and offPeakTaskId are mutually exclusive",
      });
    }
    if (payload.offPeakRunType && !payload.offPeakTaskId) {
      context.addIssue({
        code: z.ZodIssueCode.custom,
        message: "offPeakRunType requires offPeakTaskId",
        path: ["offPeakRunType"],
      });
    }
    if (payload.modelExecution && !payload.modelSelection) {
      context.addIssue({
        code: z.ZodIssueCode.custom,
        message: "modelExecution requires modelSelection",
        path: ["modelExecution"],
      });
    }
  });
export const escodeSessionSendResultSchema = z
  .object({
    sessionId: nonEmptyString,
    accepted: z.literal(true),
    stateRevision: z.number().int().nonnegative(),
  })
  .strict();
export type ESCodeSessionSendResult = z.infer<typeof escodeSessionSendResultSchema>;

export const escodeSessionHistoryTargetSchema = z.discriminatedUnion("kind", [
  z
    .object({
      kind: z.literal("turn"),
      turnIndex: z.number().int().nonnegative(),
    })
    .strict(),
  z
    .object({
      kind: z.literal("message"),
      messageId: nonEmptyString,
    })
    .strict(),
  z
    .object({
      kind: z.literal("checkpoint"),
      checkpointId: nonEmptyString,
    })
    .strict(),
  z
    .object({
      kind: z.literal("latestCheckpoint"),
    })
    .strict(),
]);
export type ESCodeSessionHistoryTarget = z.infer<typeof escodeSessionHistoryTargetSchema>;

export const escodeSessionForkParamsSchema = z
  .object({
    sessionId: nonEmptyString,
    target: escodeSessionHistoryTargetSchema.default({
      kind: "latestCheckpoint",
    }),
    expectedRevision: z.number().int().nonnegative().optional(),
  })
  .strict();
export type ESCodeSessionForkParams = z.infer<typeof escodeSessionForkParamsSchema>;

export const escodeSessionForkResultSchema = z
  .object({
    forkedSessionId: nonEmptyString,
    parentSessionId: nonEmptyString.optional(),
    targetMessageId: nonEmptyString.optional(),
    targetCheckpointId: nonEmptyString.optional(),
    response: z.string(),
    snapshot: escodeSessionStateSnapshotSchema,
  })
  .strict();
export type ESCodeSessionForkResult = z.infer<typeof escodeSessionForkResultSchema>;

export const escodeSessionCompactParamsSchema = z
  .object({
    sessionId: nonEmptyString,
    inputId: nonEmptyString.optional(),
    instructions: z.string().optional(),
    expectedRevision: z.number().int().nonnegative().optional(),
  })
  .strict();
export type ESCodeSessionCompactParams = z.infer<typeof escodeSessionCompactParamsSchema>;

export const escodeSessionCompactResultSchema = z
  .object({
    response: z.string(),
    snapshot: escodeSessionStateSnapshotSchema,
    compact: z
      .object({
        state: z.enum(["accepted", "already_running"]),
        inputId: nonEmptyString.optional(),
        operationId: nonEmptyString.optional(),
      })
      .strict()
      .optional(),
  })
  .strict();
export type ESCodeSessionCompactResult = z.infer<typeof escodeSessionCompactResultSchema>;

export const escodeSessionGoalActionSchema = z.enum([
  "show",
  "set",
  "replace",
  "pause",
  "resume",
  "clear",
]);
export type ESCodeSessionGoalAction = z.infer<typeof escodeSessionGoalActionSchema>;

export const escodeSessionGoalParamsSchema = z
  .object({
    sessionId: nonEmptyString,
    inputId: nonEmptyString.optional(),
    action: escodeSessionGoalActionSchema,
    objective: z.string().optional(),
    expectedRevision: z.number().int().nonnegative().optional(),
  })
  .strict();
export type ESCodeSessionGoalParams = z.infer<typeof escodeSessionGoalParamsSchema>;

export const escodeSessionGoalResultSchema = z
  .object({
    response: z.string(),
    snapshot: escodeSessionStateSnapshotSchema,
    startedTurn: z.boolean().optional(),
  })
  .strict();
export type ESCodeSessionGoalResult = z.infer<typeof escodeSessionGoalResultSchema>;

export const escodeSessionStopParamsSchema = z
  .object({
    sessionId: nonEmptyString,
  })
  .strict();
const escodeBackgroundTaskInfoStatusSchema = z.enum([
  "running",
  "completed",
  "failed",
  "timed_out",
  "cancelled",
  "spawn_error",
  "lost",
]);

export const escodeBackgroundTaskInfoSchema = z
  .object({
    taskId: nonEmptyString,
    toolCallId: nonEmptyString.optional(),
    toolName: nonEmptyString.optional(),
    taskKind: z.enum(["bash", "subagent"]).optional(),
    blocked: z.boolean().optional(),
    blockedReason: z.string().optional(),
    cancellable: z.boolean().optional(),
    cancelRequestedAt: protocolInstantSchema.optional(),
    command: z.string().optional(),
    description: z.string().optional(),
    status: escodeBackgroundTaskInfoStatusSchema,
    pid: z.number().int().positive().optional(),
    startedAt: protocolInstantSchema.optional(),
    completedAt: protocolInstantSchema.optional(),
    outputPath: z.string().optional(),
    stderrPersistedOutputPath: z.string().optional(),
    stdoutPersistedOutputPath: z.string().optional(),
    outputBytes: z.number().int().nonnegative().optional(),
    outputTruncated: z.boolean().optional(),
    outputTail: z.string().optional(),
    stderrBytes: z.number().int().nonnegative().optional(),
    stderrTail: z.string().optional(),
    stdoutBytes: z.number().int().nonnegative().optional(),
    stdoutTail: z.string().optional(),
    terminalId: nonEmptyString.optional(),
  })
  .strict();
export const escodeSessionCancelBackgroundTaskParamsSchema = z
  .object({
    sessionId: nonEmptyString,
    taskId: nonEmptyString,
  })
  .strict();
export type ESCodeSessionCancelBackgroundTaskParams = z.infer<
  typeof escodeSessionCancelBackgroundTaskParamsSchema
>;

export const escodeSessionCancelBackgroundTaskResultSchema = z
  .object({
    cancelled: z.boolean(),
    reason: z.string().optional(),
    snapshot: escodeBackgroundTaskInfoSchema.optional(),
    status: escodeBackgroundTaskInfoStatusSchema,
    taskId: nonEmptyString,
  })
  .strict();
export type ESCodeSessionCancelBackgroundTaskResult = z.infer<
  typeof escodeSessionCancelBackgroundTaskResultSchema
>;

export const escodeSessionSetModelParamsSchema = z
  .object({
    sessionId: nonEmptyString,
    model: modelSelectionSchema,
    expectedRevision: z.number().int().nonnegative().optional(),
    persistAsWorkspaceLastUsed: z.boolean().default(true),
  })
  .strict();
export type ESCodeSessionSetModelParams = z.infer<typeof escodeSessionSetModelParamsSchema>;

export const escodeSessionSetThoughtLevelParamsSchema = z
  .object({
    sessionId: nonEmptyString,
    thoughtLevel: nonEmptyString.optional(),
    expectedRevision: z.number().int().nonnegative().optional(),
    persistAsWorkspaceLastUsed: z.boolean().default(true),
  })
  .strict();
export type ESCodeSessionSetThoughtLevelParams = z.infer<
  typeof escodeSessionSetThoughtLevelParamsSchema
>;

export const escodeSessionSetModeParamsSchema = z
  .object({
    sessionId: nonEmptyString,
    mode: escodeSessionModeSchema,
    expectedRevision: z.number().int().nonnegative().optional(),
  })
  .strict();
export type ESCodeSessionSetModeParams = z.infer<typeof escodeSessionSetModeParamsSchema>;

export const escodeSessionCloseParamsSchema = z
  .object({
    sessionId: nonEmptyString,
    expectedPersistence: escodeSessionPersistenceSchema.optional(),
  })
  .strict();
export type ESCodeSessionCloseParams = z.infer<typeof escodeSessionCloseParamsSchema>;
export const escodeSessionCloseResultSchema = z
  .object({
    closed: z.boolean().optional(),
  })
  .strict();
export type ESCodeSessionCloseResult = z.infer<typeof escodeSessionCloseResultSchema>;
export const escodeWorkspaceReadPresentationParamsSchema = z
  .object({
    workspace: escodeWorkspaceRefSchema,
    includeExecutionCapabilities: z.boolean().optional(),
  })
  .strict();
export const escodeWorkspacePresentationSchema = z
  .object({
    workspace: escodeWorkspaceRefSchema,
    mode: escodeSessionModeSchema,
    executionCapabilities: runtimeExecutionCapabilitiesSchema.optional(),
    slashCommands: z.array(escodeSlashCommandSchema),
  })
  .strict();
export type ESCodeWorkspacePresentation = z.infer<typeof escodeWorkspacePresentationSchema>;
const workspaceHookSha256DigestSchema = z.string().regex(/^[a-f0-9]{64}$/u);
export const escodeWorkspaceHookTrustGrantParamsSchema = z
  .object({
    workspace: escodeWorkspaceRefSchema,
    bundleDigest: workspaceHookSha256DigestSchema,
    hookDeclarationDigest: workspaceHookSha256DigestSchema,
  })
  .strict();
export type ESCodeWorkspaceHookTrustGrantParams = z.infer<
  typeof escodeWorkspaceHookTrustGrantParamsSchema
>;
export const escodeWorkspaceHookTrustGrantReasonCodeSchema = z.enum([
  "workspace_hooks_blocked_by_policy",
  "workspace_hooks_bundle_changed",
  "workspace_hooks_snapshot_mismatch",
  "workspace_hooks_policy_requires_pretrust",
  "workspace_hooks_trust_store_corrupt",
  "workspace_hooks_config_unreadable",
]);
export type ESCodeWorkspaceHookTrustGrantReasonCode = z.infer<
  typeof escodeWorkspaceHookTrustGrantReasonCodeSchema
>;
export const escodeWorkspaceHookTrustGrantResultSchema = z
  .object({
    accepted: z.boolean(),
    reasonCode: escodeWorkspaceHookTrustGrantReasonCodeSchema.optional(),
  })
  .strict();
export type ESCodeWorkspaceHookTrustGrantResult = z.infer<
  typeof escodeWorkspaceHookTrustGrantResultSchema
>;
const escodeWorkspaceModelToolCallSchema = z
  .object({
    id: nonEmptyString,
    name: nonEmptyString,
    input: z.unknown(),
  })
  .strict();
const escodeWorkspaceModelMessageSchema = z.discriminatedUnion("role", [
  z.object({ role: z.literal("system"), content: z.string() }).strict(),
  z.object({ role: z.literal("user"), content: z.string() }).strict(),
  z
    .object({
      role: z.literal("assistant"),
      content: z.string(),
      toolCalls: z.array(escodeWorkspaceModelToolCallSchema).optional(),
    })
    .strict(),
  z
    .object({
      role: z.literal("tool"),
      content: z.string(),
      toolCallId: nonEmptyString,
      toolName: nonEmptyString,
      isError: z.boolean().optional(),
    })
    .strict(),
]);
const escodeWorkspaceModelToolSchema = z
  .object({
    name: nonEmptyString,
    description: z.string().optional(),
    inputSchema: z.record(z.string(), z.unknown()),
  })
  .strict();

export const escodeWorkspaceGenerateTextParamsSchema = z
  .object({
    workspace: escodeWorkspaceRefSchema,
    selection: modelSelectionSchema,
    prompt: nonEmptyString.optional(),
    messages: z.array(escodeWorkspaceModelMessageSchema).min(1).optional(),
    tools: z.array(escodeWorkspaceModelToolSchema).optional(),
    querySource: nonEmptyString,
    maxOutputTokens: z.number().int().positive().optional(),
    operationId: nonEmptyString.optional(),
  })
  .strict()
  .refine((value) => value.prompt !== undefined || value.messages !== undefined, {
    message: "prompt 或 messages 至少需要提供一个",
  });
export const escodeWorkspaceGenerateTextResultSchema = z
  .object({
    text: z.string(),
    selection: modelSelectionSchema,
    toolCalls: z.array(escodeWorkspaceModelToolCallSchema).optional(),
    // 可选以兼容仍在运行的旧 app-server；新 CLI 始终返回结构化结束原因。
    finishReason: z.string().optional(),
    usage: z
      .object({
        inputTokens: z.number().nonnegative().optional(),
        outputTokens: z.number().nonnegative().optional(),
        totalTokens: z.number().nonnegative().optional(),
        cacheReadTokens: z.number().nonnegative().optional(),
        cacheWriteTokens: z.number().nonnegative().optional(),
        reasoningTokens: z.number().nonnegative().optional(),
        serverToolUse: z
          .object({
            webSearchRequests: z.number().nonnegative().optional(),
            webFetchRequests: z.number().nonnegative().optional(),
          })
          .strict()
          .optional(),
      })
      .strict()
      .optional(),
  })
  .strict();
export type ESCodeWorkspaceGenerateTextParams = z.infer<
  typeof escodeWorkspaceGenerateTextParamsSchema
>;
export type ESCodeWorkspaceModelMessage = z.infer<typeof escodeWorkspaceModelMessageSchema>;
export type ESCodeWorkspaceModelTool = z.infer<typeof escodeWorkspaceModelToolSchema>;
export type ESCodeWorkspaceGenerateTextResult = z.infer<
  typeof escodeWorkspaceGenerateTextResultSchema
>;
export const escodeWorkspaceCancelGenerateTextParamsSchema = z
  .object({ operationId: nonEmptyString })
  .strict();
export const escodeWorkspaceCancelGenerateTextResultSchema = z
  .object({ operationId: nonEmptyString, cancelled: z.boolean() })
  .strict();

export const escodeProviderTestModelConnectivityParamsSchema = z
  .object({
    workspace: escodeWorkspaceRefSchema,
    selection: modelSelectionSchema,
  })
  .strict();
export const escodeProviderTestModelConnectivityResultSchema = z
  .object({ success: z.literal(true) })
  .strict();
export type ESCodeProviderTestModelConnectivityParams = z.infer<
  typeof escodeProviderTestModelConnectivityParamsSchema
>;
export type ESCodeProviderTestModelConnectivityResult = z.infer<
  typeof escodeProviderTestModelConnectivityResultSchema
>;

export const escodeProviderUpdateAccountConfigParamsSchema = z
  .object({
    revision: nonEmptyString,
    basedOnESCodeBuiltinRevision: nonEmptyString,
    // Provider Config 的字段校验由 @escode/provider 负责；协议层只约束可传输信封。
    providers: z.record(z.string(), z.unknown()),
    // 账号状态与 Overlay 必须一起传递，否则 Worker 会丢失非当前套餐的执行门禁。
    states: z.record(
      z.string(),
      z
        .object({
          availability: z.enum(["available", "pending", "unavailable", "unknown"]),
          entitled: z.boolean(),
          unavailableReason: accountProviderUnavailableReasonSchema.optional(),
          current: z.boolean().optional(),
          connectionKey: z.string().optional(),
          effectiveAt: z.number().finite().optional(),
        })
        .strict(),
    ),
  })
  .strict();
export const escodeProviderUpdateAccountConfigResultSchema = z
  .object({
    // 收到账号结果不代表配套 Built-in 已到达；应用版本只能读取 Registry 快照。
    receivedRevision: nonEmptyString,
    providerCount: z.number().int().nonnegative(),
    status: z.enum(["received", "unchanged"]),
  })
  .strict();
export type ESCodeProviderUpdateAccountConfigResult = z.infer<
  typeof escodeProviderUpdateAccountConfigResultSchema
>;
export const escodeInteractionPreferencesSchema = z
  .object({
    askUserQuestionAutoResolutionEnabled: z.boolean(),
  })
  .strict();
export type ESCodeInteractionPreferences = z.infer<typeof escodeInteractionPreferencesSchema>;

export const escodeWorkspaceUpdateInteractionPreferencesParamsSchema = z
  .object({
    workspace: escodeWorkspaceRefSchema,
    preferences: escodeInteractionPreferencesSchema,
  })
  .strict();
export type ESCodeWorkspaceUpdateInteractionPreferencesParams = z.infer<
  typeof escodeWorkspaceUpdateInteractionPreferencesParamsSchema
>;

export const escodeWorkspaceUpdateInteractionPreferencesResultSchema = z
  .object({
    workspace: escodeWorkspaceRefSchema,
    askUserQuestionAutoResolutionEnabled: z.boolean(),
    snoozedInteractionCount: z.number().int().nonnegative(),
  })
  .strict();
export type ESCodeWorkspaceUpdateInteractionPreferencesResult = z.infer<
  typeof escodeWorkspaceUpdateInteractionPreferencesResultSchema
>;

export const escodeModelIoPreferencesSchema = z
  .object({
    fullRetentionEnabled: z.boolean(),
  })
  .strict();
export type ESCodeModelIoPreferences = z.infer<typeof escodeModelIoPreferencesSchema>;

export const escodeWorkspaceUpdateModelIoPreferencesParamsSchema = z
  .object({
    workspace: escodeWorkspaceRefSchema,
    preferences: escodeModelIoPreferencesSchema,
  })
  .strict();
export type ESCodeWorkspaceUpdateModelIoPreferencesParams = z.infer<
  typeof escodeWorkspaceUpdateModelIoPreferencesParamsSchema
>;

export const escodeWorkspaceUpdateModelIoPreferencesResultSchema = z
  .object({
    workspace: escodeWorkspaceRefSchema,
    fullRetentionEnabled: z.boolean(),
    updatedSessionCount: z.number().int().nonnegative(),
  })
  .strict();
export type ESCodeWorkspaceUpdateModelIoPreferencesResult = z.infer<
  typeof escodeWorkspaceUpdateModelIoPreferencesResultSchema
>;

export const escodeWorkspaceUpdateOffPeakToolPolicyParamsSchema = z
  .object({
    workspace: escodeWorkspaceRefSchema,
    enabled: z.boolean(),
  })
  .strict();
export type ESCodeWorkspaceUpdateOffPeakToolPolicyParams = z.infer<
  typeof escodeWorkspaceUpdateOffPeakToolPolicyParamsSchema
>;

export const escodeWorkspaceUpdateOffPeakToolPolicyResultSchema = z
  .object({
    workspace: escodeWorkspaceRefSchema,
    enabled: z.boolean(),
  })
  .strict();
export type ESCodeWorkspaceUpdateOffPeakToolPolicyResult = z.infer<
  typeof escodeWorkspaceUpdateOffPeakToolPolicyResultSchema
>;

// 动态工作流灰度门禁：workspace 级事实，
// 与 Off-Peak 同一套 host→CLI 同步模式；旧 CLI method-not-found → host 降级忽略。
export const escodeWorkspaceUpdateDynamicWorkflowPolicyParamsSchema = z
  .object({
    workspace: escodeWorkspaceRefSchema,
    enabled: z.boolean(),
  })
  .strict();
export type ESCodeWorkspaceUpdateDynamicWorkflowPolicyParams = z.infer<
  typeof escodeWorkspaceUpdateDynamicWorkflowPolicyParamsSchema
>;

export const escodeWorkspaceUpdateDynamicWorkflowPolicyResultSchema = z
  .object({
    workspace: escodeWorkspaceRefSchema,
    enabled: z.boolean(),
  })
  .strict();
export type ESCodeWorkspaceUpdateDynamicWorkflowPolicyResult = z.infer<
  typeof escodeWorkspaceUpdateDynamicWorkflowPolicyResultSchema
>;

export const escodePermissionRequestParamsSchema = z
  .object({
    requestId: nonEmptyString,
    sessionId: nonEmptyString,
    turnId: nonEmptyString.optional(),
    toolCallId: nonEmptyString,
    toolName: nonEmptyString,
    reason: z.string(),
    riskLevel: z.enum(["low", "medium", "high", "critical"]),
    input: z.unknown(),
    origin: escodeInteractionRequestOriginSchema.optional(),
    options: z.array(escodePermissionOptionSchema).min(1),
  })
  .strict();
export type ESCodePermissionRequestParams = z.infer<typeof escodePermissionRequestParamsSchema>;

/** Agent 请求 app 枚举当前 workspace/session 可达且已完成握手的 browser backend。 */
export const escodeBrowserListParamsSchema = z
  .object({
    requestId: nonEmptyString,
    sessionId: nonEmptyString,
    turnId: nonEmptyString.optional(),
    workspaceKey: nonEmptyString,
    workspacePath: nonEmptyString,
    workspaceIdentity: nonEmptyString.optional(),
    remoteSessionId: nonEmptyString.optional(),
    clientMode: browserClientModeSchema,
    sessionContext: browserSessionContextKindSchema,
  })
  .strict();
export type ESCodeBrowserListParams = z.infer<typeof escodeBrowserListParamsSchema>;

export const escodeBrowserListResultSchema = browserBackendListResultSchema;
export type ESCodeBrowserListResult = z.infer<typeof escodeBrowserListResultSchema>;

/** Agent 把一条 browser-use 命令发送给 app 执行。 */
export const escodeBrowserExecuteParamsSchema = z
  .object({
    requestId: nonEmptyString,
    sessionId: nonEmptyString,
    turnId: nonEmptyString.optional(),
    browserId: nonEmptyString.optional(),
    browserGeneration: z.number().int().nonnegative().optional(),
    workspaceKey: nonEmptyString.optional(),
    workspacePath: nonEmptyString.optional(),
    workspaceIdentity: nonEmptyString.optional(),
    remoteSessionId: nonEmptyString.optional(),
    clientMode: browserClientModeSchema.optional(),
    sessionContext: browserSessionContextKindSchema.optional(),
    command: browserCommandSchema,
  })
  .strict();
export type ESCodeBrowserExecuteParams = z.infer<typeof escodeBrowserExecuteParamsSchema>;

// browser command result 是 app/agent 的同源协议结果；其中 duplicate_request_id 用于在真正维护
// pending/running 生命周期的边界拒绝 correlation key 冲突，不能依赖上游 UUID 概率保证。
export const escodeBrowserExecuteResultSchema = browserCommandResultSchema;
export type ESCodeBrowserExecuteResult = z.infer<typeof escodeBrowserExecuteResultSchema>;

export const escodeUserInputOptionSchema = z
  .object({
    value: nonEmptyString,
    label: nonEmptyString,
    description: z.string().optional(),
    preview: z.string().optional(),
  })
  .strict();
export const escodeUserInputQuestionSchema = z
  .object({
    question: nonEmptyString,
    header: nonEmptyString,
    options: z.array(escodeUserInputOptionSchema).min(1),
    multiSelect: z.boolean().optional(),
  })
  .strict();
export type ESCodeUserInputQuestion = z.infer<typeof escodeUserInputQuestionSchema>;

export const escodeUserInputRequestParamsSchema = z
  .object({
    requestId: nonEmptyString,
    sessionId: nonEmptyString,
    turnId: nonEmptyString.optional(),
    toolCallId: nonEmptyString.optional(),
    toolName: nonEmptyString.optional(),
    prompt: z.string().optional(),
    questions: z.array(escodeUserInputQuestionSchema).min(1).optional(),
    input: z.unknown().optional(),
    origin: escodeInteractionRequestOriginSchema.optional(),
    schema: z.unknown().optional(),
  })
  .strict();
export type ESCodeUserInputRequestParams = z.infer<typeof escodeUserInputRequestParamsSchema>;

export const escodeUserInputResponseSchema = z
  .object({
    action: z.enum(["accept", "decline", "cancel"]),
    content: jsonObjectSchema.optional(),
    reason: z.string().optional(),
  })
  .strict();
export type ESCodeUserInputResponse = z.infer<typeof escodeUserInputResponseSchema>;

export const escodeProviderRuntimeHeadersRequestReasonSchema = z.enum(["model-request"]);
export const escodeProviderRuntimeHeadersRequestParamsSchema = z
  .object({
    requestId: nonEmptyString,
    sessionId: nonEmptyString,
    turnId: nonEmptyString.optional(),
    workspace: escodeWorkspaceRefSchema,
    modelSelection: modelSelectionSchema,
    providerId: nonEmptyString,
    accountAccess: escodeProviderAccountAccessSchema.optional(),
    reason: escodeProviderRuntimeHeadersRequestReasonSchema,
  })
  .strict();
export type ESCodeProviderRuntimeHeadersRequestParams = z.infer<
  typeof escodeProviderRuntimeHeadersRequestParamsSchema
>;

/** 请求取消只作用于同 workspace/session 的这一轮凭据刷新。 */
export const escodeProviderRuntimeHeadersCancelledSchema = z
  .object({
    requestId: nonEmptyString,
    sessionId: nonEmptyString,
    workspace: escodeWorkspaceRefSchema,
  })
  .strict();
export type ESCodeProviderRuntimeHeadersCancelled = z.infer<
  typeof escodeProviderRuntimeHeadersCancelledSchema
>;

export const escodeProviderRuntimeHeadersResponseSchema = z.discriminatedUnion("headersApplied", [
  z
    .object({
      headersApplied: z.literal(true),
      // 合并重接：成功必须携带当前请求的鉴权材料，不依赖旧 Registry 已被写入。
      requestAuth: z
        .object({
          apiKey: nonEmptyString.optional(),
          headers: z.record(nonEmptyString, nonEmptyString).optional(),
        })
        .strict(),
      errorMessage: nonEmptyString.optional(),
    })
    .strict(),
  z
    .object({
      headersApplied: z.literal(false),
      errorMessage: nonEmptyString.optional(),
    })
    .strict(),
]);
export type ESCodeProviderRuntimeHeadersResponse = z.infer<
  typeof escodeProviderRuntimeHeadersResponseSchema
>;

// ── 官方 Server MCP 鉴权──
// Agent 进程不是用户身份权威：它把 (pluginId, mcpKey, targetOrigin) 报给 host，由 host
// 解析当前 Coding Plan 凭证并回传本次请求的身份头。请求侧不含任何秘密。
// 与 interaction/requestProviderRuntimeHeaders 同类：Agent 发起、host 自动响应、零 UI。
export const escodeOfficialMcpAuthHeadersRequestParamsSchema = z
  .object({
    requestId: nonEmptyString,
    workspace: escodeWorkspaceRefSchema,
    pluginId: nonEmptyString,
    mcpKey: nonEmptyString,
    targetOrigin: nonEmptyString,
  })
  .strict();
export type ESCodeOfficialMcpAuthHeadersRequestParams = z.infer<
  typeof escodeOfficialMcpAuthHeadersRequestParamsSchema
>;

/**
 * 失败原因必须可枚举，避免调用方按文本分流；因此响应不含 errorMessage。
 *
 * `official_mcp_origin_untrusted` 是 host 侧二次校验的拒绝原因：`targetOrigin` 不等于当前
 * ESCode API origin。判定只看 origin，`pluginId` / `mcpKey` 仅用于日志归属。与"未登录/无凭据"
 * 分开，才能在排查时区分"被拒绝"和"没身份"。
 */
export const escodeOfficialMcpAuthFailureReasonSchema = z.enum(
  OFFICIAL_MCP_AUTH_PORT_FAILURE_REASONS,
);

export const escodeOfficialMcpAuthHeadersResponseSchema = z.discriminatedUnion("ok", [
  z
    .object({
      ok: z.literal(true),
      headers: z.record(z.string(), z.string()),
    })
    .strict(),
  z
    .object({
      ok: z.literal(false),
      reason: escodeOfficialMcpAuthFailureReasonSchema,
    })
    .strict(),
]);
export type ESCodeOfficialMcpAuthHeadersResponse = z.infer<
  typeof escodeOfficialMcpAuthHeadersResponseSchema
>;

// ── Plugin management (list + enable/disable) ──
// 镜像 @escode/contracts 的 PluginMetadata, 仅保留 UI 需要的可序列化字段。
export const escodePluginOptionValueSchema = z.union([z.string(), z.number(), z.boolean()]);
export type ESCodePluginOptionValue = z.infer<typeof escodePluginOptionValueSchema>;
export const escodePluginScopeSchema = z.enum(["user", "workspace"]);
export type ESCodePluginScope = z.infer<typeof escodePluginScopeSchema>;
export const escodePluginHookDetailSchema = z
  .object({
    event: nonEmptyString,
    matcher: z.string().optional(),
    type: z.enum(["command", "process"]),
    command: nonEmptyString,
    args: z.array(z.string()).optional(),
    async: z.boolean().optional(),
    shell: z.union([z.literal(true), z.string()]).optional(),
    timeout: z.number().positive().optional(),
    timeoutMs: z.number().int().positive().optional(),
    statusMessage: z.string().optional(),
    sourcePath: z.string(),
    runnable: z.boolean(),
  })
  .strict();
export const escodePluginUserConfigOptionSchema = z
  .object({
    default: escodePluginOptionValueSchema.optional(),
    description: z.string().optional(),
    required: z.boolean().optional(),
    sensitive: z.boolean().optional(),
    title: z.string().optional(),
    type: z.enum(["string", "number", "boolean", "directory", "file"]).optional(),
  })
  .strict();
export type ESCodePluginUserConfigOption = z.infer<typeof escodePluginUserConfigOptionSchema>;

// 组件类型与详情弹窗/市场详情共用的分组顺序保持一致：agent / command / skill / hook / mcp。
// 注意：这三个 schema 必须定义在 escodePluginInfoSchema 之前，因为后者（.strict()）的 components 字段引用了它们。
export const escodePluginComponentKindSchema = z.enum(["agent", "command", "skill", "hook", "mcp"]);
export type ESCodePluginComponentKind = z.infer<typeof escodePluginComponentKindSchema>;

export const escodePluginComponentItemSchema = z
  .object({
    name: nonEmptyString,
    // 描述来自组件 frontmatter（SKILL.md / command / agent）或 manifest；缺失时省略，不伪造。
    description: z.string().optional(),
  })
  .strict();
export const escodePluginComponentGroupSchema = z
  .object({
    kind: escodePluginComponentKindSchema,
    items: z.array(escodePluginComponentItemSchema),
  })
  .strict();
export type ESCodePluginComponentGroup = z.infer<typeof escodePluginComponentGroupSchema>;

export const escodePluginInfoSchema = z
  .object({
    id: nonEmptyString,
    name: nonEmptyString,
    description: z.string().optional(),
    version: z.string().optional(),
    enabled: z.boolean(),
    source: nonEmptyString,
    marketplace: nonEmptyString,
    // manifest（plugin.json）的作者/主页回退字段；商店 listing 缺失时详情页信息区用它兜底。
    author: z.string().optional(),
    authorUrl: z.string().optional(),
    homepage: z.string().optional(),
    skillCount: z.number().int().nonnegative().optional(),
    skillRootCount: z.number().int().nonnegative(),
    commandRootCount: z.number().int().nonnegative(),
    // 权威组件清单（名称 + 可选描述），由 CLI 对插件根目录枚举得出，与启用态无关。
    // 详情 UI 直接展示，取代旧的「数量取协议、名称靠 UI 侧 join」脆弱方案。optional 兼容旧 payload。
    components: z.array(escodePluginComponentGroupSchema).optional(),
    declaredMcpServerNames: z.array(z.string()).optional(),
    hostMcpServerNames: z.array(z.string()).optional(),
    mcpServerNames: z.array(z.string()),
    hookDetails: z.array(escodePluginHookDetailSchema).optional(),
    rootPath: z.string(),
    userConfig: z.record(z.string(), escodePluginUserConfigOptionSchema).optional(),
    configuredOptions: z.record(z.string(), escodePluginOptionValueSchema).optional(),
    // 缺省表示 package 可用；missing 用于保留已声明但目标 Host 尚未物化的配置行。
    packageStatus: z.literal("missing").optional(),
    rootSource: escodePluginScopeSchema.optional(),
    enabledSource: escodePluginScopeSchema.optional(),
    optionSources: z.record(z.string(), escodePluginScopeSchema).optional(),
  })
  .strict();
export type ESCodePluginInfo = z.infer<typeof escodePluginInfoSchema>;

export const escodePluginDiagnosticSchema = z
  .object({
    code: z.string(),
    message: z.string(),
    severity: z.enum(["warning", "error"]).optional(),
    pluginId: z.string().optional(),
  })
  .strict();
export type ESCodePluginDiagnostic = z.infer<typeof escodePluginDiagnosticSchema>;

export const escodePluginsListParamsSchema = z
  .object({
    workspace: escodeWorkspaceRefSchema,
    configScope: escodePluginScopeSchema.optional(),
  })
  .strict();
export const escodePluginsListResultSchema = z
  .object({
    plugins: z.array(escodePluginInfoSchema),
    diagnostics: z.array(escodePluginDiagnosticSchema),
  })
  .strict();
export type ESCodePluginsListResult = z.infer<typeof escodePluginsListResultSchema>;

// ── Plugin 对话引用 catalog──
// Session-scoped 只读投影：带 sessionId → 该 Session 创建时冻结的身份 catalog；
// 不带 → workspace 当前 catalog（新建草稿 Picker）。身份与能力字段保持
// identifiers-only，不携带 rootPath/配置等；可选 icon/displayName(I18n)/description(I18n)
// 仅供 UI 展示与 Picker 搜索，不参与身份、权限或 runtime reminder。
export const escodePluginReferenceCatalogEntrySchema = z
  .object({
    // 仅 referenceCatalogWithCategory 返回；旧入口保持原结构。
    category: nonEmptyString.optional(),
    pluginId: nonEmptyString,
    name: nonEmptyString,
    marketplace: nonEmptyString,
    icon: z.string().optional(),
    // 商店 listing 的 display-only 本地化显示名投影（沿 icon 先例）：让 Picker 能按
    // 中文显示名搜索/展示；locale 解析复用 shared 的 plugin-display-name helper。
    displayName: z.string().optional(),
    displayNameI18n: z.record(z.string(), z.string()).optional(),
    // 仅供 Picker 展示，不进入能力身份或 model-only reminder。
    description: z.string().optional(),
    descriptionI18n: z.record(z.string(), z.string()).optional(),
    enabled: z.boolean(),
    // 非空 = 与其他 enabled Plugin 共享 manifest name 的 V1 fail closed 冲突：
    // Picker 禁选并展示原因，runtime 解析按 ambiguous 跳过。
    conflictingPluginIds: z.array(nonEmptyString),
    skillQualifiedNames: z.array(nonEmptyString),
    mcpServerNames: z.array(nonEmptyString),
    // 旧 Host 不投影该字段时按空数组兼容；只有新 Agent 会把它用于 reminder live 交集。
    subagentNames: z.array(nonEmptyString).default([]),
  })
  .strict();
export type ESCodePluginReferenceCatalogEntry = z.infer<
  typeof escodePluginReferenceCatalogEntrySchema
>;

export const escodePluginsReferenceCatalogParamsSchema = z
  .object({
    workspace: escodeWorkspaceRefSchema,
    // 已有 Session 的 Picker 必须带 sessionId 才能拿到 session-owned catalog；
    // session 不存在时按协议错误 fail closed，禁止静默回退 workspace authority。
    sessionId: nonEmptyString.optional(),
  })
  .strict();
export type ESCodePluginsReferenceCatalogParams = z.infer<
  typeof escodePluginsReferenceCatalogParamsSchema
>;
export const escodePluginsReferenceCatalogResultSchema = z
  .object({
    authority: z.enum(["session", "workspace"]),
    plugins: z.array(escodePluginReferenceCatalogEntrySchema),
  })
  .strict();
export type ESCodePluginsReferenceCatalogResult = z.infer<
  typeof escodePluginsReferenceCatalogResultSchema
>;

// ── Skill 对话引用 catalog──
// 新草稿读取 workspace 当前目录；已有 Session 读取 AgentRuntime 首次 context
// 初始化时冻结的发现结果。该协议只承载 Composer 的只读引用投影，不替代 Settings
// 的 Skill 管理接口，也不持久化 runtime 快照。
export const escodeSkillReferenceCatalogEntrySchema = z
  .object({
    id: nonEmptyString,
    name: nonEmptyString,
    description: z.string(),
    path: nonEmptyString,
    scope: z.enum(["workspace", "user", "plugin"]),
    enabled: z.literal(true),
    pluginName: nonEmptyString.optional(),
  })
  .strict();
export type ESCodeSkillReferenceCatalogEntry = z.infer<typeof escodeSkillReferenceCatalogEntrySchema>;

export const escodeSkillsReferenceCatalogParamsSchema = z
  .object({
    workspace: escodeWorkspaceRefSchema,
    // 带 sessionId 时必须命中该进程内的 resident Session；未知 Session fail closed，
    // 禁止回退到 workspace 当前目录而把新 Skill 泄漏进旧对话。
    sessionId: nonEmptyString.optional(),
  })
  .strict();
export type ESCodeSkillsReferenceCatalogParams = z.infer<
  typeof escodeSkillsReferenceCatalogParamsSchema
>;
export const escodeSkillsReferenceCatalogResultSchema = z
  .object({
    authority: z.enum(["session", "workspace"]),
    skills: z.array(escodeSkillReferenceCatalogEntrySchema),
  })
  .strict();
export type ESCodeSkillsReferenceCatalogResult = z.infer<
  typeof escodeSkillsReferenceCatalogResultSchema
>;

// ── 已保存工作流的 GUI 中枢──
// workspace 级、无会话的五个方法，照 skills/referenceCatalog 的先例：每次调用现扫
// `<cwd>/.escode/workflows/`（挂载时快照会漏掉手改的文件）。形状与 @escode/contracts 的
// saved-workflow.ts 逐字对齐——依赖方向是 contracts → shared，所以这里结构化地再声明一遍，
// 而不是 import；两边的 strict 形状由 bootstrap 侧的协议测试互相钉住。
export const escodeSavedWorkflowArgTypeSchema = z.enum(["string", "number", "boolean", "json"]);
export type ESCodeSavedWorkflowArgType = z.infer<typeof escodeSavedWorkflowArgTypeSchema>;
export const escodeSavedWorkflowArgDeclarationSchema = z
  .object({
    type: escodeSavedWorkflowArgTypeSchema,
    description: z.string().optional(),
    required: z.boolean().optional(),
    default: z.unknown().optional(),
  })
  .strict();
export type ESCodeSavedWorkflowArgDeclaration = z.infer<
  typeof escodeSavedWorkflowArgDeclarationSchema
>;
export const escodeSavedWorkflowArgsDeclarationSchema = z.record(
  z.string(),
  escodeSavedWorkflowArgDeclarationSchema,
);
export type ESCodeSavedWorkflowArgsDeclaration = z.infer<
  typeof escodeSavedWorkflowArgsDeclarationSchema
>;
export const escodeSavedWorkflowMetaSchema = z
  .object({
    description: nonEmptyString,
    whenToUse: nonEmptyString.optional(),
    args: escodeSavedWorkflowArgsDeclarationSchema.optional(),
  })
  .strict();
export type ESCodeSavedWorkflowMeta = z.infer<typeof escodeSavedWorkflowMetaSchema>;
// 作用域两档：项目档落 `<cwd>/.escode/workflows/`、全局档落 agent 机器的 `~/.escode/workflows/`。作用域由文件所在目录推得，frontmatter 不存 scope。
export const escodeSavedWorkflowScopeSchema = z.enum(["project", "global"]);
export type ESCodeSavedWorkflowScope = z.infer<typeof escodeSavedWorkflowScopeSchema>;
export const escodeSavedWorkflowEntrySchema = z
  .object({
    name: nonEmptyString,
    description: z.string(),
    whenToUse: z.string().optional(),
    args: escodeSavedWorkflowArgsDeclarationSchema.optional(),
    scope: escodeSavedWorkflowScopeSchema,
    path: nonEmptyString,
  })
  .strict();
export type ESCodeSavedWorkflowEntry = z.infer<typeof escodeSavedWorkflowEntrySchema>;
export const escodeSavedWorkflowInvalidEntrySchema = z
  .object({ path: nonEmptyString, reason: nonEmptyString })
  .strict();
export type ESCodeSavedWorkflowInvalidEntry = z.infer<typeof escodeSavedWorkflowInvalidEntrySchema>;
/** 名字非法 / 未找到 / frontmatter 坏 / 读错——与 core store 的 resolve 失败四态逐字对应。 */
export const escodeSavedWorkflowFailureReasonSchema = z.enum([
  "invalid_name",
  "not_found",
  "parse_error",
  "read_error",
]);
export type ESCodeSavedWorkflowFailureReason = z.infer<typeof escodeSavedWorkflowFailureReasonSchema>;
const escodeSavedWorkflowFailureSchema = z
  .object({
    ok: z.literal(false),
    reason: escodeSavedWorkflowFailureReasonSchema,
    detail: z.string().optional(),
  })
  .strict();

export const escodeWorkflowsListParamsSchema = z
  .object({
    workspace: escodeWorkspaceRefSchema,
    // 缺省即 `project`（本项目档）。给 `global` 时改扫本机 `~/.escode/workflows/`；此时 `workspace`
    // 仍必填，但只是**载体运行时**——协议处理器对全局档不读它的路径。
    scope: escodeSavedWorkflowScopeSchema.optional(),
  })
  .strict();
export type ESCodeWorkflowsListParams = z.infer<typeof escodeWorkflowsListParamsSchema>;
export const escodeWorkflowsListResultSchema = z
  .object({
    workflows: z.array(escodeSavedWorkflowEntrySchema),
    invalid: z.array(escodeSavedWorkflowInvalidEntrySchema),
    // 扫过的目录（本地绝对路径），即使目录还不存在也回：GUI 的文件监听靠它 watch。
    dir: nonEmptyString,
  })
  .strict();
export type ESCodeWorkflowsListResult = z.infer<typeof escodeWorkflowsListResultSchema>;

export const escodeWorkflowsGetParamsSchema = z
  .object({
    workspace: escodeWorkspaceRefSchema,
    name: nonEmptyString,
    // 缺省 `project`；`global` 时只查本机全局根。`workspace` 语义同 list（全局档只当载体）。
    scope: escodeSavedWorkflowScopeSchema.optional(),
  })
  .strict();
export type ESCodeWorkflowsGetParams = z.infer<typeof escodeWorkflowsGetParamsSchema>;
export const escodeWorkflowsGetResultSchema = z.union([
  z
    .object({
      ok: z.literal(true),
      name: nonEmptyString,
      path: nonEmptyString,
      scope: escodeSavedWorkflowScopeSchema,
      meta: escodeSavedWorkflowMetaSchema,
      /** 脚本本体（frontmatter 之后逐字节），即被类型检查与执行的那一份。 */
      script: z.string(),
    })
    .strict(),
  escodeSavedWorkflowFailureSchema,
]);
export type ESCodeWorkflowsGetResult = z.infer<typeof escodeWorkflowsGetResultSchema>;

export const escodeWorkflowsUpdateMetaParamsSchema = z
  .object({
    workspace: escodeWorkspaceRefSchema,
    name: nonEmptyString,
    meta: escodeSavedWorkflowMetaSchema,
    // 缺省 `project`；`global` 时只写本机全局根那一份。`workspace` 语义同 list。
    scope: escodeSavedWorkflowScopeSchema.optional(),
  })
  .strict();
export type ESCodeWorkflowsUpdateMetaParams = z.infer<typeof escodeWorkflowsUpdateMetaParamsSchema>;
export const escodeWorkflowsUpdateMetaResultSchema = z.union([
  z.object({ ok: z.literal(true), path: nonEmptyString }).strict(),
  escodeSavedWorkflowFailureSchema,
]);
export type ESCodeWorkflowsUpdateMetaResult = z.infer<typeof escodeWorkflowsUpdateMetaResultSchema>;

export const escodeWorkflowsDeleteParamsSchema = z
  .object({
    workspace: escodeWorkspaceRefSchema,
    name: nonEmptyString,
    // 缺省 `project`；`global` 时按 scope 选根删除（不再写死 roots[0]）。`workspace` 语义同 list。
    scope: escodeSavedWorkflowScopeSchema.optional(),
  })
  .strict();
export type ESCodeWorkflowsDeleteParams = z.infer<typeof escodeWorkflowsDeleteParamsSchema>;
export const escodeWorkflowsDeleteResultSchema = z.union([
  z.object({ ok: z.literal(true), path: nonEmptyString }).strict(),
  escodeSavedWorkflowFailureSchema,
]);
export type ESCodeWorkflowsDeleteResult = z.infer<typeof escodeWorkflowsDeleteResultSchema>;

export const ESCODE_WORKFLOWS_RUNS_MAX_LIMIT = 50;
export const escodeWorkflowsRunsParamsSchema = z
  .object({
    workspace: escodeWorkspaceRefSchema,
    /** 只要这个名字的 run（`dwf_run.name` 字面等值）；缺省即本项目全部 run。 */
    name: nonEmptyString.optional(),
    limit: z.number().int().min(1).max(ESCODE_WORKFLOWS_RUNS_MAX_LIMIT),
    // 缺省 `project`：只查 `dwf_run.cwd === workspacePath` 的 run。`global` 时**不**按 cwd 过滤，
    // 跨所有项目取该名字的运行历史（全局工作流在任何项目里跑，历史因此跨 cwd）；结果行带 `cwd`
    // 供 GUI 标项目。`workspace` 语义同 list（全局档只当载体）。
    scope: escodeSavedWorkflowScopeSchema.optional(),
  })
  .strict();
export type ESCodeWorkflowsRunsParams = z.infer<typeof escodeWorkflowsRunsParamsSchema>;
// 三终态词汇：errored = 脚本之错，stopped = 被停下（可恢复）。
export const escodeSavedWorkflowRunStatusSchema = z.enum([
  "pending",
  "running",
  "completed",
  "errored",
  "stopped",
]);
export type ESCodeSavedWorkflowRunStatus = z.infer<typeof escodeSavedWorkflowRunStatusSchema>;
export const escodeSavedWorkflowRunStopReasonSchema = z.enum([
  "user",
  "model",
  "provider",
  "interrupted",
  "superseded",
]);
export const escodeSavedWorkflowRunSchema = z
  .object({
    runId: nonEmptyString,
    name: z.string().optional(),
    status: escodeSavedWorkflowRunStatusSchema,
    // `status === "stopped"` 才在场。
    stopReason: escodeSavedWorkflowRunStopReasonSchema.optional(),
    createdAt: z.number(),
    updatedAt: z.number(),
    spentTokens: z.number(),
    /** 发起它的会话与 CreateWorkflow 工具调用：有这两个才能从中枢打开实例详情。老行可缺。 */
    parentSessionId: z.string().optional(),
    toolCallId: z.string().optional(),
    args: z.record(z.string(), z.unknown()).optional(),
    // 实际运行的项目目录（`dwf_run.cwd`）。全局档的 `workflows/runs` 跨 cwd 查询，GUI 用它给
    // 每行标项目；项目档变体里它恒等于 workspacePath，GUI 可忽略。老行可缺。
    cwd: z.string().optional(),
    // 这次运行发布的**用户面产物**：中枢的运行历史行在
    // 状态词之后画一串 kind chips，详情页头部的「最近产物」条取最近一次 completed run 的这一份。
    // ⚠ 术语：这里的 artifact 是脚本经 `artifact.*` 发布给用户看的产出，不是脚本的顶层返回值。
    // 只带 chip 画得下的字段（≤ 8 件，取最新版的元数据）；字节与条目经 v4 查询按需读。
    // optional，照上面 `cwd` 的先例：老 CLI 不发，少一个键是退化不是错误。
    artifacts: z
      .array(
        z
          .object({
            id: nonEmptyString,
            kind: z.enum(["file", "markdown", "chart", "table", "metrics", "board"]),
            title: z.string().optional(),
            version: z.number(),
            contentType: z.string().optional(),
          })
          .strict(),
      )
      .max(8)
      .optional(),
  })
  .strict();
export type ESCodeSavedWorkflowRun = z.infer<typeof escodeSavedWorkflowRunSchema>;
export const escodeWorkflowsRunsResultSchema = z
  .object({
    runs: z.array(escodeSavedWorkflowRunSchema),
    /** 为真时才在场：还有更多 run 没进这一页（多取一条判定，不是 length === limit）。 */
    truncated: z.literal(true).optional(),
  })
  .strict();
export type ESCodeWorkflowsRunsResult = z.infer<typeof escodeWorkflowsRunsResultSchema>;

// workflows/move：把本机全局根的同名文件搬到 `workspace` 项目根。**只此一向**：项目→全局不是搬文件而是模型的概括（「提升为
// 全局」在该项目开新会话、经 SaveWorkflow 另存），所以没有 `to` 参数。同机同用户，rename 优先、EXDEV
// 回落 copy+unlink；逐字节搬，不改内容（frontmatter 不存 scope）；`move` 不覆盖——目标已存在即拒绝
// （覆盖是 SaveWorkflow 经确认窗才有的动作，不变式 7）。`workspace` 既是载体运行时也是目标项目。
export const escodeWorkflowsMoveParamsSchema = z
  .object({
    workspace: escodeWorkspaceRefSchema,
    name: nonEmptyString,
  })
  .strict();
export type ESCodeWorkflowsMoveParams = z.infer<typeof escodeWorkflowsMoveParamsSchema>;
export const escodeWorkflowsMoveResultSchema = z.union([
  z
    .object({
      ok: z.literal(true),
      /** 源落点路径（全局根，搬走前）。 */
      from: nonEmptyString,
      /** 目标落点路径（项目根，搬到处）。 */
      to: nonEmptyString,
    })
    .strict(),
  z
    .object({
      ok: z.literal(false),
      // target_exists：目标档已有同名（move 不覆盖）；not_found：源档没有这个名字；
      // read_error / write_error：搬运时的 I/O 失败；invalid_name：名字先验没过。
      reason: z.enum(["invalid_name", "not_found", "target_exists", "read_error", "write_error"]),
      path: z.string().optional(),
      detail: z.string().optional(),
    })
    .strict(),
]);
export type ESCodeWorkflowsMoveResult = z.infer<typeof escodeWorkflowsMoveResultSchema>;

// 推荐 Prompt 的可信插件解析：UI 不拆解 stableId，也不从旧目录快照推断可安装性。
export const escodePluginSuggestedReferenceStatusSchema = z.enum([
  "ready",
  "disabled",
  "missing",
  "conflict",
  "unavailable",
]);
export type ESCodePluginSuggestedReferenceStatus = z.infer<
  typeof escodePluginSuggestedReferenceStatusSchema
>;
export const escodePluginOperationStateSchema = z.enum([
  "checking",
  "refreshing",
  "installing",
  "enabling",
  "cancelling",
  "cancelled",
  "complete",
  "failed",
]);
export type ESCodePluginOperationState = z.infer<typeof escodePluginOperationStateSchema>;
export const escodePluginOperationProgressNotificationSchema = z
  .object({
    operationId: nonEmptyString,
    state: z.literal("refreshing"),
  })
  .strict();
export type ESCodePluginOperationProgressNotification = z.infer<
  typeof escodePluginOperationProgressNotificationSchema
>;
export const escodePluginsResolveSuggestedReferenceParamsSchema = z
  .object({
    workspace: escodeWorkspaceRefSchema,
    stableId: nonEmptyString,
    operationId: nonEmptyString,
    clientMode: escodeDeliveryKindSchema,
    deliveryKind: escodeDeliveryKindSchema,
  })
  .strict();
export type ESCodePluginsResolveSuggestedReferenceParams = z.infer<
  typeof escodePluginsResolveSuggestedReferenceParamsSchema
>;
export const escodePluginsSetEnabledParamsSchema = z
  .object({
    workspace: escodeWorkspaceRefSchema,
    pluginId: nonEmptyString,
    enabled: z.boolean(),
    operationId: nonEmptyString.optional(),
    scope: escodePluginScopeSchema.optional(),
  })
  .strict();
export const escodePluginsSetEnabledResultSchema = z
  .object({
    plugin: escodePluginInfoSchema,
    enabled: z.boolean(),
  })
  .strict();
export type ESCodePluginsSetEnabledResult = z.infer<typeof escodePluginsSetEnabledResultSchema>;

// 商店信息（Store Listing）：目录条目携带的展示性元数据（显示名/icon/分类/作者/链接/hero/
// 示例提示词），全部可选，UI 缺失时按降级矩阵处理（字母头像/隐藏区块/省略信息行）。
// i18n 采用 `<字段>I18n` map，locale 解析复用 shared 的 plugin-display-name helper。
export const escodePluginStoreListingSchema = z
  .object({
    displayName: z.string().optional(),
    displayNameI18n: z.record(z.string(), z.string()).optional(),
    descriptionI18n: z.record(z.string(), z.string()).optional(),
    icon: z.string().optional(),
    category: z.string().optional(),
    author: z.string().optional(),
    authorUrl: z.string().optional(),
    homepage: z.string().optional(),
    privacyPolicy: z.string().optional(),
    termsOfService: z.string().optional(),
    heroImage: z.string().optional(),
    examplePrompts: z.array(z.string()).optional(),
    examplePromptsI18n: z.record(z.string(), z.array(z.string())).optional(),
    /**
     * 需要付费套餐才好用的插件：市场目录条目声明 `requiresPaidPlan: true`，
     * UI 在标题右侧展示提示图标。描述的是「使用条件」而非「插件是收费商品」——
     * 不参与安装门禁与计费，命名也不绑定具体套餐商品名。
     */
    requiresPaidPlan: z.boolean().optional(),
  })
  .strict();
export type ESCodePluginStoreListing = z.infer<typeof escodePluginStoreListingSchema>;

export const escodePluginsResolveSuggestedReferenceResultSchema = z
  .object({
    stableId: nonEmptyString,
    status: escodePluginSuggestedReferenceStatusSchema,
    marketplace: nonEmptyString.optional(),
    pluginName: nonEmptyString.optional(),
    sourceTrust: z.literal("official").optional(),
    // 官方 Marketplace listing 的可选展示投影；不参与身份、安装或权限判断。
    icon: z.string().optional(),
    listing: escodePluginStoreListingSchema.optional(),
    diagnostics: z.array(escodePluginDiagnosticSchema),
  })
  .strict()
  .superRefine((value, context) => {
    if (value.status !== "ready" && value.status !== "disabled" && value.status !== "missing") {
      return;
    }
    if (!value.marketplace || !value.pluginName || value.sourceTrust !== "official") {
      context.addIssue({
        code: "custom",
        message: "actionable suggested Plugin results require trusted install identity",
      });
    }
  });
export type ESCodePluginsResolveSuggestedReferenceResult = z.infer<
  typeof escodePluginsResolveSuggestedReferenceResultSchema
>;

export const escodePluginMarketplaceSummarySchema = z
  .object({
    id: nonEmptyString,
    name: nonEmptyString,
    source: jsonObjectSchema,
    description: z.string().optional(),
    lastUpdated: z.string().optional(),
    pluginCount: z.number().int().nonnegative(),
    isOfficial: z.boolean().optional(),
    // 目录顶层 featured 策展名单（商店「公开」分段 Featured 区）。
    featured: z.array(z.string()).optional(),
    refreshFailure: z
      .object({
        code: z.string(),
        failedAt: z.string(),
        message: z.string(),
      })
      .strict()
      .optional(),
  })
  .strict();
export type ESCodePluginMarketplaceSummary = z.infer<typeof escodePluginMarketplaceSummarySchema>;

export const escodeAvailablePluginSummarySchema = z
  .object({
    id: nonEmptyString,
    name: nonEmptyString,
    marketplace: nonEmptyString,
    description: z.string().optional(),
    version: z.string().optional(),
    installed: z.boolean(),
    componentTypes: z.array(z.string()).optional(),
    listing: escodePluginStoreListingSchema.optional(),
  })
  .strict();
export type ESCodeAvailablePluginSummary = z.infer<typeof escodeAvailablePluginSummarySchema>;

export const escodeInstalledPluginSummarySchema = z
  .object({
    id: nonEmptyString,
    name: nonEmptyString,
    marketplace: nonEmptyString,
    description: z.string().optional(),
    version: z.string().optional(),
    enabled: z.boolean(),
    scope: escodePluginScopeSchema,
    installPath: z.string().optional(),
    installedAt: z.string().optional(),
    componentTypes: z.array(z.string()).optional(),
    hookDetails: z.array(escodePluginHookDetailSchema).optional(),
    updateStatus: z.enum(["none", "update-available", "version-changed"]).optional(),
    latestVersion: z.string().optional(),
    listing: escodePluginStoreListingSchema.optional(),
  })
  .strict();
export type ESCodeInstalledPluginSummary = z.infer<typeof escodeInstalledPluginSummarySchema>;

export const escodePluginsOverviewParamsSchema = z
  .object({
    workspace: escodeWorkspaceRefSchema,
    configScope: escodePluginScopeSchema.optional(),
  })
  .strict();
export const escodePluginsOverviewResultSchema = z
  .object({
    marketplaces: z.array(escodePluginMarketplaceSummarySchema),
    availablePlugins: z.array(escodeAvailablePluginSummarySchema),
    installedPlugins: z.array(escodeInstalledPluginSummarySchema),
    restorableBuiltins: z.array(escodeAvailablePluginSummarySchema),
    diagnostics: z.array(escodePluginDiagnosticSchema),
    capability: z
      .object({
        supported: z.boolean(),
        reason: z.string().optional(),
      })
      .strict(),
  })
  .strict();
export type ESCodePluginsOverviewResult = z.infer<typeof escodePluginsOverviewResultSchema>;

export const escodePluginsMarketplaceAddParamsSchema = z
  .object({
    workspace: escodeWorkspaceRefSchema,
    source: nonEmptyString,
    dryRun: z.boolean().optional(),
    operationId: nonEmptyString.optional(),
  })
  .strict();
export const escodePluginsMarketplaceRemoveParamsSchema = z
  .object({
    workspace: escodeWorkspaceRefSchema,
    marketplace: nonEmptyString,
  })
  .strict();
export const escodePluginsMarketplaceUpdateParamsSchema = z
  .object({
    workspace: escodeWorkspaceRefSchema,
    marketplace: nonEmptyString.optional(),
    operationId: nonEmptyString.optional(),
  })
  .strict();
export const escodePluginsMarketplaceMutationResultSchema = z
  .object({
    marketplace: escodePluginMarketplaceSummarySchema.optional(),
    marketplaces: z.array(escodePluginMarketplaceSummarySchema).optional(),
    diagnostics: z.array(escodePluginDiagnosticSchema).optional(),
  })
  .strict();
export type ESCodePluginsMarketplaceMutationResult = z.infer<
  typeof escodePluginsMarketplaceMutationResultSchema
>;

export const escodePluginsInstallParamsSchema = z
  .object({
    workspace: escodeWorkspaceRefSchema,
    pluginName: nonEmptyString,
    marketplace: nonEmptyString,
    scope: escodePluginScopeSchema.optional(),
    dryRun: z.boolean().optional(),
    operationId: nonEmptyString.optional(),
  })
  .strict();
export const escodePluginsCancelOperationParamsSchema = z
  .object({
    operationId: nonEmptyString,
  })
  .strict();
export type ESCodePluginsCancelOperationParams = z.infer<
  typeof escodePluginsCancelOperationParamsSchema
>;

export const escodePluginsCancelOperationResultSchema = z
  .object({
    operationId: nonEmptyString,
    cancelled: z.boolean(),
  })
  .strict();
export type ESCodePluginsCancelOperationResult = z.infer<
  typeof escodePluginsCancelOperationResultSchema
>;
export const escodePluginsUninstallParamsSchema = z
  .object({
    workspace: escodeWorkspaceRefSchema,
    pluginId: nonEmptyString.optional(),
    pluginName: nonEmptyString.optional(),
    marketplace: nonEmptyString.optional(),
    removeCache: z.boolean().optional(),
  })
  .strict();
export const escodePluginsInstallResultSchema = z
  .object({
    installedPlugins: z.array(escodeInstalledPluginSummarySchema),
    dependencyClosure: z.array(z.string()),
    diagnostics: z.array(escodePluginDiagnosticSchema),
  })
  .strict();
export type ESCodePluginsInstallResult = z.infer<typeof escodePluginsInstallResultSchema>;

export const escodePluginsUninstallResultSchema = z
  .object({
    removedPlugin: escodeInstalledPluginSummarySchema.optional(),
    diagnostics: z.array(escodePluginDiagnosticSchema),
  })
  .strict();
export type ESCodePluginsUninstallResult = z.infer<typeof escodePluginsUninstallResultSchema>;

export const escodePluginsUpdateParamsSchema = z
  .object({
    workspace: escodeWorkspaceRefSchema,
    pluginId: nonEmptyString.optional(),
    marketplace: nonEmptyString.optional(),
  })
  .strict();
export const escodePluginsRestoreBuiltinParamsSchema = z
  .object({
    workspace: escodeWorkspaceRefSchema,
    pluginId: nonEmptyString,
  })
  .strict();
export const escodePluginsRestoreBuiltinResultSchema = z
  .object({
    pluginId: nonEmptyString,
    diagnostics: z.array(escodePluginDiagnosticSchema),
  })
  .strict();
export type ESCodePluginsRestoreBuiltinResult = z.infer<
  typeof escodePluginsRestoreBuiltinResultSchema
>;

export const escodePluginsConfigureParamsSchema = z
  .object({
    workspace: escodeWorkspaceRefSchema,
    pluginId: nonEmptyString,
    options: jsonObjectSchema,
    clearOptionKeys: z.array(nonEmptyString).optional(),
    scope: escodePluginScopeSchema.optional(),
    dryRun: z.boolean().optional(),
  })
  .strict();
export const escodePluginsConfigureResultSchema = z
  .object({
    pluginId: nonEmptyString,
    diagnostics: z.array(escodePluginDiagnosticSchema),
  })
  .strict();
export type ESCodePluginsConfigureResult = z.infer<typeof escodePluginsConfigureResultSchema>;

export const escodePluginsResetConfigParamsSchema = z
  .object({
    workspace: escodeWorkspaceRefSchema,
    pluginId: nonEmptyString,
    scope: escodePluginScopeSchema.optional(),
  })
  .strict();
export type ESCodePluginsResetConfigParams = z.infer<typeof escodePluginsResetConfigParamsSchema>;

export const escodePluginsValidateParamsSchema = z
  .object({
    workspace: escodeWorkspaceRefSchema,
    pluginName: nonEmptyString.optional(),
    marketplace: nonEmptyString.optional(),
    source: nonEmptyString.optional(),
  })
  .strict();
export const escodePluginsValidateResultSchema = z
  .object({
    ok: z.boolean(),
    diagnostics: z.array(escodePluginDiagnosticSchema),
    compatibility: z
      .object({
        runnable: z.array(z.string()),
        diagnosticOnly: z.array(z.string()),
        unsupported: z.array(z.string()),
      })
      .strict(),
  })
  .strict();
export type ESCodePluginsValidateResult = z.infer<typeof escodePluginsValidateResultSchema>;

// plugins/describe：按需枚举单个插件的组件「名称 + 描述」。
// 已安装插件读本地缓存目录；未安装候选按需解析/临时 clone 源后枚举再清理。
export const escodePluginsDescribeParamsSchema = z
  .object({
    workspace: escodeWorkspaceRefSchema,
    pluginName: nonEmptyString,
    marketplace: nonEmptyString,
  })
  .strict();
export const escodePluginsDescribeResultSchema = z
  .object({
    components: z.array(escodePluginComponentGroupSchema),
    diagnostics: z.array(escodePluginDiagnosticSchema).optional(),
    // 插件包内 plugin.json 的展示性回退字段；未安装候选详情页信息区在商店 listing 缺失时兜底。
    metadata: z
      .object({
        author: z.string().optional(),
        authorUrl: z.string().optional(),
        homepage: z.string().optional(),
        version: z.string().optional(),
      })
      .strict()
      .optional(),
  })
  .strict();
export type ESCodePluginsDescribeResult = z.infer<typeof escodePluginsDescribeResultSchema>;

export const escodeAutomationScheduleRuleSchema = z
  .object({
    unit: z.enum(["minute", "hourly", "daily", "weekly", "monthly", "yearly"]),
    interval: z.number().int().positive(),
    hour: z.number().int().min(0).max(23),
    minute: z.number().int().min(0).max(59),
    anchorAt: z.number().int(),
    weekdays: z.array(z.number().int().min(0).max(6)).optional(),
    monthDays: z.array(z.number().int().min(1).max(31)).optional(),
    /** yearly 用：1-12 人类月份。缺省回退 anchorAt 的月份（兼容未写该字段的旧记录）。 */
    months: z.array(z.number().int().min(1).max(12)).optional(),
    monthlyMode: z.enum(["date", "weekday"]).optional(),
  })
  .strict();
export type ESCodeAutomationScheduleRuleProtocol = z.infer<typeof escodeAutomationScheduleRuleSchema>;

/** 会话侧长间隔周期 carrier 的 unit 枚举（与 scheduleRule.unit 同集）。 */
export const escodeAutomationIntervalUnitSchema = z.enum([
  "minute",
  "hourly",
  "daily",
  "weekly",
  "monthly",
  "yearly",
]);

export const escodeAutomationProtocolSchema = z
  .object({
    automationId: nonEmptyString,
    title: z.string(),
    cronExpr: nonEmptyString,
    prompt: nonEmptyString,
    modelSelection: modelSelectionSchema.optional(),
    mode: escodeTaskModeSchema.optional(),
    targetTaskId: nonEmptyString.optional(),
    enabled: z.boolean(),
    lifecycleStatus: z.enum(["active", "completed", "failed", "paused"]),
    nextRunAt: timestampMsSchema.optional(),
    lastRunAt: timestampMsSchema.optional(),
    runCount: z.number().int().nonnegative(),
    recurring: z.boolean(),
    maxRuns: z.number().int().positive().optional(),
    // 自定义重复规则；缺省时调度回退到解析 cronExpr。会话卡片必须读到本字段才能展示
    // cron 无法表达的真实间隔（如每50小时、每40天，兼容 cronExpr 只是 0 * * * *）。
    scheduleRule: escodeAutomationScheduleRuleSchema.optional(),
  })
  .strict();
export type ESCodeAutomationProtocol = z.infer<typeof escodeAutomationProtocolSchema>;

export const escodeAutomationCreateParamsSchema = z
  .object({
    title: z.string().optional(),
    cronExpr: nonEmptyString,
    relativeDelayMinutes: z.number().int().positive().max(525_600).optional(),
    prompt: nonEmptyString,
    modelSelection: modelSelectionSchema.optional(),
    mode: escodeTaskModeSchema.optional(),
    targetTaskId: nonEmptyString.optional(),
    botDeliveryTarget: escodeAutomationBotDeliveryTargetSchema.optional(),
    recurring: z.boolean().optional(),
    maxRuns: z.number().int().positive().optional(),
    // 会话侧自定义重复 carrier：每 N 分钟/小时/天/周/月/年均通过此字段归一化为权威 scheduleRule，
    // cronExpr 仅作合法兼容展示。
    intervalUnit: escodeAutomationIntervalUnitSchema.optional(),
    interval: z.number().int().min(1).max(200).optional(),
  })
  .strict()
  // intervalUnit 与 interval 必须配对提交（只传一个无法确定真实间隔）。
  .refine((input) => (input.intervalUnit === undefined) === (input.interval === undefined), {
    message: "intervalUnit and interval must be set together",
    path: ["interval"],
  })
  // 周期 carrier 与一次性相对延迟语义冲突，禁止同传。
  .refine((input) => input.intervalUnit === undefined || input.relativeDelayMinutes === undefined, {
    message: "intervalUnit cannot combine with a relative delayMinutes",
    path: ["intervalUnit"],
  })
  .refine((input) => input.intervalUnit === undefined || input.recurring !== false, {
    message: "intervalUnit is a recurring carrier and cannot combine with recurring=false",
    path: ["recurring"],
  })
  .refine((input) => input.intervalUnit === undefined || input.maxRuns === undefined, {
    message: "intervalUnit is a recurring carrier and cannot combine with maxRuns",
    path: ["maxRuns"],
  });
export type ESCodeAutomationCreateProtocolParams = z.infer<typeof escodeAutomationCreateParamsSchema>;

export const escodeAutomationCreateResultSchema = z
  .object({ automation: escodeAutomationProtocolSchema })
  .strict();
export type ESCodeAutomationCreateProtocolResult = z.infer<typeof escodeAutomationCreateResultSchema>;

export const escodeAutomationUpdateParamsSchema = z
  .object({
    automationId: nonEmptyString,
    title: nonEmptyString.optional(),
    cronExpr: nonEmptyString.optional(),
    prompt: nonEmptyString.optional(),
    recurring: z.boolean().optional(),
    maxRuns: z.number().int().positive().nullable().optional(),
    // 会话侧自定义重复 carrier（同 create 侧语义）。
    intervalUnit: escodeAutomationIntervalUnitSchema.optional(),
    interval: z.number().int().min(1).max(200).optional(),
  })
  .strict()
  .refine(
    (input) =>
      input.title !== undefined ||
      input.cronExpr !== undefined ||
      input.prompt !== undefined ||
      input.recurring !== undefined ||
      input.maxRuns !== undefined ||
      input.intervalUnit !== undefined,
    { message: "automation update requires at least one field" },
  )
  .refine((input) => input.maxRuns !== null || input.recurring === true, {
    message: "clearing maxRuns requires recurring=true",
    path: ["maxRuns"],
  })
  .refine((input) => input.recurring !== true || typeof input.maxRuns !== "number", {
    message: "recurring=true cannot be combined with a numeric maxRuns",
    path: ["maxRuns"],
  })
  // intervalUnit 与 interval 必须配对提交（同 create 侧语义）。
  .refine((input) => (input.intervalUnit === undefined) === (input.interval === undefined), {
    message: "intervalUnit and interval must be set together",
    path: ["interval"],
  })
  .refine((input) => input.intervalUnit === undefined || input.recurring !== false, {
    message: "intervalUnit is a recurring carrier and cannot combine with recurring=false",
    path: ["recurring"],
  })
  .refine(
    (input) =>
      input.intervalUnit === undefined ||
      input.maxRuns === undefined ||
      (input.maxRuns === null && input.recurring === true),
    {
      message:
        "intervalUnit is a recurring carrier and only allows maxRuns=null with recurring=true",
      path: ["maxRuns"],
    },
  );
export type ESCodeAutomationUpdateProtocolParams = z.infer<typeof escodeAutomationUpdateParamsSchema>;
export const escodeAutomationUpdateResultSchema = z
  .object({ automation: escodeAutomationProtocolSchema })
  .strict();
export type ESCodeAutomationUpdateProtocolResult = z.infer<typeof escodeAutomationUpdateResultSchema>;

export const escodeAutomationListParamsSchema = z.object({}).strict();
export type ESCodeAutomationListProtocolParams = z.infer<typeof escodeAutomationListParamsSchema>;
export const escodeAutomationListResultSchema = z
  .object({ automations: z.array(escodeAutomationProtocolSchema) })
  .strict();
export type ESCodeAutomationListProtocolResult = z.infer<typeof escodeAutomationListResultSchema>;

export const escodeAutomationCheckTaskBindingParamsSchema = z
  .object({ targetTaskId: nonEmptyString })
  .strict();
export type ESCodeAutomationCheckTaskBindingProtocolParams = z.infer<
  typeof escodeAutomationCheckTaskBindingParamsSchema
>;
export const escodeAutomationCheckTaskBindingResultSchema = z
  .object({ bound: z.boolean() })
  .strict();
export type ESCodeAutomationCheckTaskBindingProtocolResult = z.infer<
  typeof escodeAutomationCheckTaskBindingResultSchema
>;

export const escodeAutomationDeleteParamsSchema = z
  .object({ automationId: nonEmptyString })
  .strict();
export type ESCodeAutomationDeleteProtocolParams = z.infer<typeof escodeAutomationDeleteParamsSchema>;
export const escodeAutomationDeleteResultSchema = z.object({ deleted: z.boolean() }).strict();
export type ESCodeAutomationDeleteProtocolResult = z.infer<typeof escodeAutomationDeleteResultSchema>;

// ---- Off-Peak（闲时任务）会话内创建协议----
// 与 automation 兄弟并列（独立域，禁止互相复用标记/表）。workspace 由 host 端从
// 当前 session 注入，不进协议参数（对称 automation/create）。permissionMode 只开放产品
// 四档词表；缺省解析在 host 端（yolo / allowed_models 末位 / 最高推理档）。
export const escodeOffPeakPermissionModeSchema = z.enum(["build", "edit", "plan", "yolo"]);
export type ESCodeOffPeakProtocolPermissionMode = z.infer<typeof escodeOffPeakPermissionModeSchema>;

export const escodeOffPeakCreateParamsSchema = z
  .object({
    title: nonEmptyString,
    prompt: nonEmptyString,
    permissionMode: escodeOffPeakPermissionModeSchema.optional(),
    model: nonEmptyString.optional(),
    thoughtLevel: nonEmptyString.optional(),
    // 会话内创建绑定当前会话（对齐 automation/create 的 targetTaskId），由 CLI 端口填入。
    boundSessionId: nonEmptyString.optional(),
  })
  .strict();
export type ESCodeOffPeakCreateProtocolParams = z.infer<typeof escodeOffPeakCreateParamsSchema>;

// 协议侧任务快照：轮尾卡片与 OffPeakList 的最小字段面。
// 不暴露 serverTicketId（跨边界禁带）。
export const escodeOffPeakTaskSnapshotSchema = z
  .object({
    offPeakTaskId: nonEmptyString,
    title: z.string(),
    status: z.enum(["queued", "paused", "running", "completed", "failed", "cancelled"]),
    queuePosition: z.number().int().positive().optional(),
    sessionId: nonEmptyString.optional(),
    createdAt: z.number().int().nonnegative(),
  })
  .strict();
export type ESCodeOffPeakTaskProtocolSnapshot = z.infer<typeof escodeOffPeakTaskSnapshotSchema>;

// 失败分类跨协议保真（镜像 shared OffPeakTaskCreateResult 的判别联合，错误不降级为字符串）。
// model 白名单预校失败复用 client_validation 分类 + errorCode "model_not_allowed"，不扩分类枚举。
export const escodeOffPeakCreateResultSchema = z.discriminatedUnion("ok", [
  z.object({ ok: z.literal(true), task: escodeOffPeakTaskSnapshotSchema }).strict(),
  z
    .object({
      ok: z.literal(false),
      failureStage: z.enum(["client_validation", "ticket_request", "local_persist"]),
      errorCategory: z.enum([
        "client_validation",
        "eligibility_3101",
        "quota_3103",
        "network",
        "invalid_response",
        "local_persist",
        "unknown",
      ]),
      errorCode: z.string(),
    })
    .strict(),
]);
export type ESCodeOffPeakCreateProtocolResult = z.infer<typeof escodeOffPeakCreateResultSchema>;

export const escodeOffPeakListParamsSchema = z.object({}).strict();
export type ESCodeOffPeakListProtocolParams = z.infer<typeof escodeOffPeakListParamsSchema>;
export const escodeOffPeakListResultSchema = z
  .object({ tasks: z.array(escodeOffPeakTaskSnapshotSchema) })
  .strict();
export type ESCodeOffPeakListProtocolResult = z.infer<typeof escodeOffPeakListResultSchema>;

export const escodeProtocolMethods = {
  runtimeCapabilities: "runtime/capabilities",
  computerUseOperationEvent: "computer-use/operation-event",
  sessionCreate: "session/create",
  sessionResume: "session/resume",
  sessionList: "session/list",
  sessionSubagents: "session/subagents",
  sessionRequestRuntimePreferences: "session/requestRuntimePreferences",
  sessionRead: "session/read",
  sessionMessages: "session/messages",
  sessionEvents: "session/events",
  sessionDebug: "session/debug",
  sessionSubscribe: "session/subscribe",
  // @deprecated（部分）：send 主路径已收敛 v4 sendText；仅剩 adapter 附件
  // 回退分支消费（v4 attachmentRef 上传/寄存命令面未建模），待附件命令面落地后移除。
  sessionSend: "session/send",
  // @deprecated：host 客户端方法已删（stop 已收敛 v4 stop 命令）。
  // wire case 留兼容（transport bypass 名单仍引用），随旧词整体删除时一并移除。
  sessionStop: "session/stop",
  // @deprecated：host 客户端方法已删（已收敛 v4 cancelBackgroundWork 命令）。
  // wire case 留兼容，随旧词整体删除时一并移除。
  sessionCancelBackgroundTask: "session/cancelBackgroundTask",
  // @deprecated：host 客户端方法已删（v4 forkAssistant 原生 handler 经
  // forkSessionAtMessage 钩子直调 server-operations.forkSession op）。wire case 与
  // fork params/result schema 保留＝op 存活面；fork record 归 v4 原生重写。
  sessionFork: "session/fork",
  sessionCompact: "session/compact",
  sessionGoal: "session/goal",
  sessionClose: "session/close",
  // setModel 仍被 escodeSessionService 的 desktop 旧链路消费；replayable
  // switchModelConfig 已直接由目标 Environment Registry 解析 Selection。
  sessionSetModel: "session/setModel",
  // replayable facade 的思考深度/模式已收敛 v4 switchModelConfig/
  // switchCollaborationMode；剩余消费 = escodeSessionService（desktop 旧链路，随
  // 桌面 v4 UI 收口清零）与 setMode 的 auto 值残留（v4 值域刻意排除 auto）。
  sessionSetThoughtLevel: "session/setThoughtLevel",
  sessionSetMode: "session/setMode",
  workspaceReadPresentation: "workspace/readPresentation",
  workspaceHookTrustGrant: "workspace/hooks/trustGrant",
  // 进程级 Account Provider Config 与 workspace 运行目录分离。
  providerUpdateAccountConfig: "provider/updateAccountConfig",
  workspaceUpdateInteractionPreferences: "workspace/updateInteractionPreferences",
  workspaceUpdateModelIoPreferences: "workspace/updateModelIoPreferences",
  // Off-Peak 工具面门禁是 workspace 级事实（灰度 + 本地/远程），由 host 在 agent 就绪时同步；
  // CLI 对 legacy create/resume 与 v4 冷恢复统一读取。旧 CLI method-not-found → host 降级忽略。
  workspaceUpdateOffPeakToolPolicy: "workspace/updateOffPeakToolPolicy",
  // 动态工作流灰度门禁：同 Off-Peak 的同步模式。
  workspaceUpdateDynamicWorkflowPolicy: "workspace/updateDynamicWorkflowPolicy",
  // LLM 执行面在 CLI，直连不可行；消费仅 services 内部
  // （commit message），待 v4 workspace 查询/命令面覆盖后移除。
  workspaceGenerateText: "workspace/generateText",
  workspaceCancelGenerateText: "workspace/cancelGenerateText",
  providerTestModelConnectivity: "provider/testModelConnectivity",
  mcpList: "mcp/list",
  pluginsList: "plugins/list",
  pluginsReferenceCatalog: "plugins/referenceCatalog",
  pluginsReferenceCatalogWithCategory: "plugins/referenceCatalogWithCategory",
  skillsReferenceCatalog: "skills/referenceCatalog",
  // 已保存工作流的 GUI 中枢：workspace 级、无会话。
  workflowsList: "workflows/list",
  workflowsGet: "workflows/get",
  workflowsUpdateMeta: "workflows/updateMeta",
  workflowsDelete: "workflows/delete",
  workflowsRuns: "workflows/runs",
  // 在项目档 / 全局档之间移动同名文件。
  workflowsMove: "workflows/move",
  pluginsResolveSuggestedReference: "plugins/resolveSuggestedReference",
  pluginsSetEnabled: "plugins/setEnabled",
  pluginsOverview: "plugins/overview",
  pluginsMarketplaceAdd: "plugins/marketplace/add",
  pluginsMarketplaceRemove: "plugins/marketplace/remove",
  pluginsMarketplaceUpdate: "plugins/marketplace/update",
  pluginsInstall: "plugins/install",
  pluginsCancelOperation: "plugins/cancelOperation",
  pluginsUninstall: "plugins/uninstall",
  pluginsUpdate: "plugins/update",
  pluginsRestoreBuiltin: "plugins/restoreBuiltin",
  pluginsConfigure: "plugins/configure",
  pluginsResetConfig: "plugins/resetConfig",
  pluginsValidate: "plugins/validate",
  pluginsDescribe: "plugins/describe",
  automationCreate: "automation/create",
  automationUpdate: "automation/update",
  automationCheckTaskBinding: "automation/checkTaskBinding",
  automationList: "automation/list",
  automationDelete: "automation/delete",
  // Off-Peak 会话内创建：与 automation 兄弟并列的独立方法族。
  offPeakCreate: "offPeak/create",
  offPeakList: "offPeak/list",
  // @deprecated：host 消费已清零（escodeAgentService 改走 v4/usage/stats）。
  // 仅剩 CLI server 的 wire 兼容 case；随旧词整体删除时一并移除。
  usageStats: "usage/stats",
  // ESCode Protocol 对 agent 只暴露 session-first 方法；task 是 UI 投影概念，不能泄露进协议方法名。
  // @deprecated：host 已改走 v4/conversation/usage；后续与 usage/stats 一并移除。
  sessionUsage: "session/usage",
  // 资源管理器：CLI 回报其 MCP 子进程 pid 与插件归属（纯内存，无 I/O），采样在 Host 侧完成。
  processChildProcesses: "process/childProcesses",
  interactionRequestPermission: "interaction/requestPermission",
  interactionRequestUserInput: "interaction/requestUserInput",
  interactionRequestProviderRuntimeHeaders: "interaction/requestProviderRuntimeHeaders",
  interactionRequestOfficialMcpAuthHeaders: "interaction/requestOfficialMcpAuthHeaders",
  // browser-use 反向请求由 agent 发起，host 转给 main 中的 CDP executor。
  interactionBrowserList: "interaction/browserList",
  interactionBrowserExecute: "interaction/browserExecute",
  // Agent 串口工具：agent broker 转发 MCP serial 工具调用，host 交给 Desktop Local Host 的 SerialService。
  interactionSerialList: escodeSerialMethods.list,
  interactionSerialOpen: escodeSerialMethods.open,
  interactionSerialWrite: escodeSerialMethods.write,
  interactionSerialRead: escodeSerialMethods.read,
  interactionSerialWaitFor: escodeSerialMethods.waitFor,
  interactionSerialClose: escodeSerialMethods.close,
  interactionSerialSetSignals: escodeSerialMethods.setSignals,
  interactionSerialCancel: escodeSerialCancelMethod,
} as const;

export type ESCodeProtocolMethod = (typeof escodeProtocolMethods)[keyof typeof escodeProtocolMethods];

export const escodeProtocolEmptyResultSchema = z.object({}).strict();

// 最新 V4 主链已不再依赖旧版全量方法表；这里仅保留仍被兼容测试和 browser broker
// 消费的最小契约集合，避免重新引入已移除的 legacy 方法。
export const escodeProtocolSessionMethodContracts = {
  [escodeProtocolMethods.workspaceHookTrustGrant]: {
    params: escodeWorkspaceHookTrustGrantParamsSchema,
    result: escodeWorkspaceHookTrustGrantResultSchema,
  },
  [escodeProtocolMethods.mcpList]: {
    params: escodeMcpListParamsSchema,
    result: escodeMcpListResultSchema,
  },
  [escodeProtocolMethods.interactionBrowserList]: {
    params: escodeBrowserListParamsSchema,
    result: escodeBrowserListResultSchema,
  },
  [escodeProtocolMethods.interactionBrowserExecute]: {
    params: escodeBrowserExecuteParamsSchema,
    result: escodeBrowserExecuteResultSchema,
  },
  [escodeProtocolMethods.interactionSerialList]: {
    params: escodeSerialMethodParamsSchemas.list,
    result: escodeSerialMethodResultSchemas.list,
  },
  [escodeProtocolMethods.interactionSerialOpen]: {
    params: escodeSerialMethodParamsSchemas.open,
    result: escodeSerialMethodResultSchemas.open,
  },
  [escodeProtocolMethods.interactionSerialWrite]: {
    params: escodeSerialMethodParamsSchemas.write,
    result: escodeSerialMethodResultSchemas.write,
  },
  [escodeProtocolMethods.interactionSerialRead]: {
    params: escodeSerialMethodParamsSchemas.read,
    result: escodeSerialMethodResultSchemas.read,
  },
  [escodeProtocolMethods.interactionSerialWaitFor]: {
    params: escodeSerialMethodParamsSchemas.waitFor,
    result: escodeSerialMethodResultSchemas.waitFor,
  },
  [escodeProtocolMethods.interactionSerialClose]: {
    params: escodeSerialMethodParamsSchemas.close,
    result: escodeSerialMethodResultSchemas.close,
  },
  [escodeProtocolMethods.interactionSerialSetSignals]: {
    params: escodeSerialMethodParamsSchemas.setSignals,
    result: escodeSerialMethodResultSchemas.setSignals,
  },
  [escodeProtocolMethods.interactionSerialCancel]: {
    params: escodeSerialCancelParamsSchema,
    result: escodeProtocolEmptyResultSchema,
  },
} as const satisfies Partial<
  Record<ESCodeProtocolMethod, { params: z.ZodTypeAny; result: z.ZodTypeAny }>
>;

export type ESCodeProtocolSessionMethodContract =
  (typeof escodeProtocolSessionMethodContracts)[keyof typeof escodeProtocolSessionMethodContracts];

/** 仅存储准备子进程的私有控制帧，原始路径不进入业务事件或遥测。 */
export const escodeStoragePreparationFrameSchema = z.discriminatedUnion("method", [
  z
    .object({
      method: z.literal("startup/storagePath"),
      params: z.object({ path: z.string().min(1).max(32768) }).strict(),
    })
    .strict(),
  z
    .object({ method: z.literal("startup/storagePrepared"), params: z.object({}).strict() })
    .strict(),
  z
    .object({ method: z.literal("startup/storageState"), params: escodeStorageStartupStateSchema })
    .strict(),
]);
export const escodeStoragePathReadySchema = z
  .object({ method: z.literal("startup/storagePathReady"), reuse: z.boolean().optional() })
  .strict();
export * from "../localTtft.js";

// 桌面本地 TTFT 的严格事实合同；检查点不能替代实际内容帧。
export { localTtftFactsSchema } from "../localTtft.js";
