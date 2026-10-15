// ESCode Host 任务快照投影对拍用的 agent 协议快照夹具（固定输入，供生成器与 Rust 测试使用）。
// 覆盖：多轮 user/assistant、reasoning、三种工具状态、compact summary、model-only synthetic user、
// goal 续跑提醒与迭代号、附件（image）、syntheticTimeline（fork/compaction）、todos/todoGroups/
// goalStats、pendingPermissions（含 user-input-backed → elicitation）、后台任务控制项。

export const WORKSPACE = "/w";
export const TASK_ID = "task_proj";

function agentMessage(messageId, role, created, parts, extra = {}) {
  return { info: { messageId, sessionId: TASK_ID, role, time: { created }, ...extra }, parts };
}

function assistantInfo(created, completed, extra = {}) {
  return {
    time: { created, ...(completed === undefined ? {} : { completed }) },
    agent: "main",
    parentMessageId: "u1",
    path: { cwd: WORKSPACE, root: WORKSPACE },
    cost: 0,
    tokens: { input: 0, output: 0, reasoning: 0, cache: { read: 0, write: 0 } },
    ...extra,
  };
}

const VISIBLE_ASSISTANT = {
  semantics: {
    origin: "agent_runtime",
    kind: "assistant_response",
    uiVisibility: "visible",
    providerVisibility: "visible",
    transcriptVisibility: "visible",
  },
};

function fixtureMessages() {
  return [
    agentMessage(
      "u1",
      "user",
      1000,
      [
        { partId: "p-u1-1", messageId: "u1", sessionId: TASK_ID, type: "text", text: "把迁移做完" },
        {
          partId: "p-u1-2",
          messageId: "u1",
          sessionId: TASK_ID,
          type: "file",
          mime: "image/png",
          filename: "shot.png",
          url: "data:image/png;base64,QUJD",
          metadata: { sizeBytes: 3 },
        },
      ],
      {
        semantics: {
          origin: "real_user",
          kind: "user_prompt",
          uiVisibility: "visible",
          providerVisibility: "hidden",
          transcriptVisibility: "visible",
        },
      },
    ),
    agentMessage(
      "a1",
      "assistant",
      1100,
      [
        { partId: "p-a1-1", messageId: "a1", sessionId: TASK_ID, type: "reasoning", text: "先看计划。" },
        { partId: "p-a1-2", messageId: "a1", sessionId: TASK_ID, type: "text", text: "先读文件。" },
        {
          partId: "p-a1-3",
          messageId: "a1",
          sessionId: TASK_ID,
          type: "tool",
          callId: "call_read_1",
          tool: "Read",
          state: {
            status: "completed",
            input: { path: "/w/a.ts" },
            output: "export const a = 1;",
            title: "Read a.ts",
            metadata: { toolCallId: "call_read_1", display: "a.ts" },
            startedAt: 1150,
            completedAt: 1200,
          },
        },
      ],
      assistantInfo(1100, 1500, { model: { providerId: "glm", modelId: "glm-4" }, finish: "stop", ...VISIBLE_ASSISTANT }),
    ),
    // goal 续跑提醒：模型上下文，不是用户可见输入（但推进迭代号）。
    agentMessage("u2", "user", 1600, [
      {
        partId: "p-u2-1",
        messageId: "u2",
        sessionId: TASK_ID,
        type: "text",
        text: '<system-reminder source="goal-continuation">Continue working toward the active session goal.</system-reminder>',
      },
    ], { synthetic: true, source: "goal-continuation" }),
    agentMessage(
      "a2",
      "assistant",
      1700,
      [{ partId: "p-a2-1", messageId: "a2", sessionId: TASK_ID, type: "text", text: "第一轮完成。" }],
      assistantInfo(1700, 1900, { model: { providerId: "glm", modelId: "glm-4" }, finish: "stop", ...VISIBLE_ASSISTANT }),
    ),
    // compact summary：模型上下文，可见投影必须过滤。
    agentMessage("u3", "user", 2000, [
      { partId: "p-u3-1", messageId: "u3", sessionId: TASK_ID, type: "compaction", auto: true, metadata: {} },
      { partId: "p-u3-2", messageId: "u3", sessionId: TASK_ID, type: "text", text: "（压缩摘要）" },
    ]),
    agentMessage("u4", "user", 2100, [
      { partId: "p-u4-1", messageId: "u4", sessionId: TASK_ID, type: "text", text: "第二轮问题" },
    ], {
      semantics: {
        origin: "real_user",
        kind: "user_prompt",
        uiVisibility: "visible",
        providerVisibility: "hidden",
        transcriptVisibility: "visible",
      },
    }),
    agentMessage(
      "a3",
      "assistant",
      2200,
      [
        { partId: "p-a3-1", messageId: "a3", sessionId: TASK_ID, type: "text", text: "开始执行。" },
        {
          partId: "p-a3-2",
          messageId: "a3",
          sessionId: TASK_ID,
          type: "tool",
          callId: "call_bash_1",
          tool: "Bash",
          state: {
            status: "error",
            input: { command: "false" },
            error: "exit 1",
            startedAt: 2250,
            completedAt: 2260,
          },
        },
      ],
      assistantInfo(2200, undefined, { model: { providerId: "glm", modelId: "glm-4" }, ...VISIBLE_ASSISTANT }),
    ),
    // 子代理消息：可见投影过滤（source=subagent_message）。
    agentMessage("u5", "user", 2300, [
      { partId: "p-u5-1", messageId: "u5", sessionId: TASK_ID, type: "text", text: "子代理汇报：完成一半。" },
    ], { synthetic: true, source: "subagent_message" }),
    // 携带 timelineStatus 的 compaction：assistant 承载的 divider，参与投影但不参与合并。
    agentMessage(
      "a4",
      "assistant",
      2400,
      [
        {
          partId: "p-a4-1",
          messageId: "a4",
          sessionId: TASK_ID,
          type: "compaction",
          auto: false,
          reason: "manual",
          metadata: { timelineStatus: "completed", trigger: "manual", operationId: "op-1" },
        },
      ],
      assistantInfo(2400, 2450, { ...VISIBLE_ASSISTANT }),
    ),
    agentMessage(
      "a5",
      "assistant",
      2500,
      [{ partId: "p-a5-1", messageId: "a5", sessionId: TASK_ID, type: "text", text: "尾部回复。" }],
      assistantInfo(2500, 2600, { model: { providerId: "custom", modelId: "kimi-k2:free", options: { reasoningLevel: "high" } }, finish: "stop", ...VISIBLE_ASSISTANT }),
    ),
    // 后台任务通知：synthetic 且带任务通知前缀 → 模型上下文；同时给工具 raw 挂通知。
    agentMessage("u6", "user", 2700, [
      {
        partId: "p-u6-1",
        messageId: "u6",
        sessionId: TASK_ID,
        type: "text",
        text: "<task-notification><tool-use-id>call_bash_1</tool-use-id><status>killed</status><summary>被停止</summary></task-notification>",
      },
    ], { synthetic: true, source: "task_notification" }),
  ];
}

export function fixtureSnapshot() {
  return {
    protocol: { name: "ESCode Protocol", version: 1 },
    session: {
      sessionId: TASK_ID,
      traceId: "trace-proj-1",
      title: "",
      createdAt: 900,
      updatedAt: 2800,
      mode: "guarded",
      model: { providerId: "glm", modelId: "glm-4" },
      status: "running",
      sessionKind: "main",
      workspace: { workspaceKey: WORKSPACE, workspacePath: WORKSPACE },
    },
    settings: {
      model: {
        current: { providerId: "glm", modelId: "glm-4", options: { reasoningLevel: "high" } },
        available: [
          {
            ref: { providerId: "glm", modelId: "glm-4" },
            label: "GLM-4",
            description: "主力模型",
            providerLabel: "智谱",
            reasoning: { levels: [{ value: "none", label: "None" }, { value: "high", label: "High" }], defaultLevel: "high" },
          },
          { ref: { providerId: "custom", modelId: "kimi-k2:free" }, label: "Kimi" },
        ],
      },
      thoughtLevel: {
        enabled: true,
        current: "high",
        defaultLevel: "medium",
        available: [{ value: "none", label: "None" }, { value: "high", label: "High", description: "深想" }],
      },
      mode: { current: "guarded" },
      permission: { mode: "guarded" },
    },
    projection: {
      sessionId: TASK_ID,
      status: "running",
      mode: "guarded",
      turnCount: 3,
      totalTokenCount: 3400,
      contextUsed: 1200,
      contextWindow: 8000,
      activeToolCalls: [
        { toolCallId: "call_bash_1", toolName: "Bash", status: "running", startedAt: 2250 },
      ],
      pendingPermissions: [
        {
          requestId: "req_bash",
          toolName: "Bash",
          reason: "需要执行命令",
          options: [{ value: "once", label: "Once" }],
          origin: "runtime",
          input: { command: "ls" },
        },
        {
          requestId: "req_ask",
          toolName: "AskUserQuestion",
          reason: "需要选择",
          origin: "runtime",
          input: {
            questions: [
              {
                question: "继续吗？",
                header: "确认",
                multiSelect: true,
                options: [{ label: "继续", description: "继续迁移" }, { label: "暂停" }],
              },
            ],
          },
        },
        {
          requestId: "req_plan",
          toolName: "ExitPlanMode",
          reason: "计划审批",
          origin: "runtime",
          input: { plan: "  1. 落投影\n2. 接索引  " },
        },
      ],
      backgroundJobs: [
        { jobId: "job_bash", taskKind: "bash", command: "npm run build", status: "running", pid: 4242, startedAt: 2200 },
        { toolName: "Task", description: "跑一遍迁移", status: "in_progress", taskId: "job_agent", cancellable: true },
      ],
      target: {
        sessionID: TASK_ID,
        targetID: "t1",
        objective: "把迁移做完",
        summaryTitle: "迁移",
        status: "active",
        tokenBudget: 1000,
        tokensUsed: 10,
        timeUsedSeconds: 5,
        time: { created: 900, updated: 1000 },
      },
    },
    runtime: {
      eventSeq: 12,
      stateRevision: 5,
      pendingRequestIds: ["req_bash"],
      activeTurnKind: "regular",
      activeTurnId: "turn-1",
      contextUsage: { used: 1300, size: 8000, cost: 0.5, cache: { readTokens: 100 }, breakdown: { system: 10 } },
      goalVerifications: { passed: 1, failed: 0 },
      goalVerificationTimeline: [
        {
          type: "goal_verification",
          targetId: "t1",
          goalIteration: 1,
          status: "completed",
          startedAt: 1800,
          updatedAt: 1850,
          anchorAssistantMessageId: "a2",
          verification: { passed: true },
        },
      ],
    },
    messages: fixtureMessages(),
    todos: [
      { status: "completed", content: "步骤一" },
      { status: "in_progress", content: "步骤二" },
    ],
    todoGroups: [
      {
        id: "g1",
        source: "agent",
        goalIteration: 1,
        targetId: "t1",
        startedAt: 1000,
        updatedAt: 1100,
        todos: [{ status: "completed", content: "组内一" }],
      },
    ],
    goalStats: { tokensUsed: 10, timeUsedSeconds: 5 },
    slashCommands: [{ name: "compact", description: "压缩上下文", source: "builtin" }],
  };
}

