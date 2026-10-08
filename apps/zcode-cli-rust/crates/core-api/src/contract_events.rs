use crate::{ModelFailure, RetryState, ToolOutput};
use anyhow::Result;
use serde_json::Value;
use tokio::sync::{mpsc, oneshot};

pub struct RunEvent {
    pub session_id: String,
    pub run_id: String,
    pub event: Event,
}
pub enum Event {
    FilePrepared {
        change: zcode_cli_domain::file_checkpoint::FileCheckpoint,
        committed: oneshot::Sender<()>,
    },
    Subagent {
        name: String,
        args: Value,
        call_id: String,
        profile: Option<Box<zcode_cli_domain::subagent::Profile>>,
        selection: Option<crate::ModelIdentity>,
        reply: oneshot::Sender<std::result::Result<crate::ChildHandle, String>>,
    },
    GoalStep {
        reply: oneshot::Sender<Option<zcode_cli_domain::goal::Goal>>,
    },
    GoalVerdict {
        target_id: String,
        verdict: zcode_cli_domain::goal::Verdict,
        usage: Value,
        reply: oneshot::Sender<Option<(zcode_cli_domain::goal::Goal, Value)>>,
    },
    SkillsInitialized {
        catalog: zcode_cli_domain::skills::SkillCatalog,
        reply: oneshot::Sender<zcode_cli_domain::skills::SkillCatalog>,
    },
    Todos {
        call_id: String,
        write: Option<Vec<zcode_cli_domain::todo::TodoItem>>,
        reply: oneshot::Sender<ToolOutput>,
    },
    TodoReminder {
        reply: oneshot::Sender<Value>,
    },
    /// hooks 追加的上下文（docs/specs/rust-hooks.md）：`Some` 落进会话消息（`before_input` 时插在本轮输入之前），
    /// `None` 撤回本轮输入（UserPromptSubmit 阻止继续）。
    HookContext {
        message: Option<Value>,
        before_input: bool,
        committed: oneshot::Sender<()>,
    },
    /// 恢复会话首轮判定出的 shell 提醒（docs/specs/rust-shell-resume-notice.md）：`at` 是相对本次运行
    /// 消息窗口（`messages[context.offset..]`）的锚点；owner 只记在内存，不落库。
    ShellNotice { at: usize, message: Value },
    /// 断流恢复开始（docs/specs/rust-model-retry.md）：owner 作废本次 assistant 尾部并显示恢复态。
    StreamRecovery {
        retry_number: u32,
        max_retries: u32,
        reason_code: &'static str,
    },
    /// 会话终端 shell 偏好（Host `integratedTerminalShell` 原值）；会话 owner 负责请求与缓存。
    ShellPreference {
        reply: oneshot::Sender<Option<Value>>,
    },
    /// Host 记忆开关与本会话已解析的记忆（docs/specs/rust-project-memory.md）；会话 owner 负责请求与缓存。
    MemoryPreference {
        reply: oneshot::Sender<(bool, Option<crate::ProjectMemory>)>,
    },
    /// 本会话首次解析出的记忆根与索引，由会话 owner 缓存供后续轮次复用。
    MemoryResolved(crate::ProjectMemory),
    /// 主轮次成功完成后的记忆提取快照，由会话 owner 的提取调度处理。
    MemoryExtract(Box<crate::MemorySnapshot>),
    /// ReadSessionContext 读取目标会话的已提交历史（会话 owner 负责访问存储）。
    SessionContext {
        id: String,
        reply: oneshot::Sender<
            anyhow::Result<Option<zcode_cli_domain::session_context::SessionSource>>,
        >,
    },
    /// `ListWorkflowRuns` 读 run journal（docs/specs/rust-dynamic-workflow.md 第 6 期）：工具在
    /// 会话回合里执行，而 journal 由会话 owner 的存储持有——经 owner 读，投影在工具侧。
    WorkflowRunList {
        query: zcode_cli_domain::dwf_journal::RunQuery,
        reply: oneshot::Sender<anyhow::Result<Vec<zcode_cli_domain::dwf_journal::JournalRun>>>,
    },
    /// 工具经会话 owner 向 Host 发起反向请求（automation/* 等）。
    HostRequest {
        method: String,
        params: Value,
        reply: HostReply,
    },
    /// 冻结会话标题（CronCreate 成功后以 automation 标题为准，titleSource=custom）。
    FreezeTitle(String),
    /// 标题 sidecar 的结果（docs/specs/rust-session-title.md）：由会话 owner 校验后写回。
    /// `title` 为空表示跳过（失败/空标题/工具调用）；有目标时 owner 改写兜底摘要标题。
    SessionTitle {
        session: String,
        /// 首条输入实体 id：会话回退后不再写回会话标题。
        entity: String,
        title: Option<String>,
        write_session: bool,
        goal_target: Option<String>,
    },
    ToolCleanupFailed(String),
    PromptInitialized {
        snapshot: Box<zcode_cli_domain::prompt::PromptSnapshot>,
        skills: zcode_cli_domain::skills::SkillCatalog,
        committed: oneshot::Sender<zcode_cli_domain::skills::SkillCatalog>,
    },
    AuxiliaryDone {
        result: std::result::Result<Value, ModelFailure>,
    },
    /// 中枢直接启动已保存工作流的启动轮（docs/specs/rust-v4-command-gaps.md「startSavedWorkflow」）：
    /// 工作流宿主已 `port.submit`（零会话副作用），会话 owner 落启动轮（标题 / userInput /
    /// controlOnly turnHeader / runtime history）并以本次 `v4/command` 的 ACK 经 `reply` 应答。
    /// 会话状态的唯一写入点在 owner，辅助任务因此不能自己落行。
    WorkflowLaunchTurn {
        session: String,
        command: Box<zcode_cli_protocol::Command>,
        launched: Value,
        reply: oneshot::Sender<Value>,
    },
    /// GUI「配置」修订工作流 run 的设置轮（docs/specs/rust-v4-command-gaps.md「amendWorkflowRunSettings」）：
    /// 工作流宿主已 `port.amend`（零会话副作用，或就地调并发），会话 owner 落设置轮（标题 / userInput /
    /// controlOnly turnHeader / runtime history），忙时改入 `Session::settings_turns` 延迟落行。ACK 经
    /// `reply` 应答。会话状态的唯一写入点在 owner，辅助任务因此不能自己落行。
    WorkflowSettingsTurn {
        session: String,
        command: Box<zcode_cli_protocol::Command>,
        applied: Value,
        reply: oneshot::Sender<Value>,
    },
    /// 后台工作区作业（插件安装 / 市场刷新等）的协议回复：错误按原文回给 Host。
    AuxiliaryReply {
        result: std::result::Result<Value, String>,
    },
    /// 工作流宿主报告一个后台 run 已结算（经工具层 Host 通道）：`notice` 是
    /// `{taskId, toolCallId, status, text, originMeta}`，会话空闲时作为后台结果轮注入（docs/specs/rust-dynamic-workflow.md M1）。
    WorkflowSettled {
        session: String,
        notice: Value,
    },
    /// 工作流宿主对 actor 会话的请求（`actor.create` / `actor.turn` / `actor.cancel` / `actor.close` /
    /// `actor.tools`，docs/specs/rust-dynamic-workflow.md「M2 设计」）：会话 owner 执行并经 `reply` 应答。
    ActorRequest {
        method: String,
        params: Value,
        reply: oneshot::Sender<std::result::Result<Value, String>>,
    },
    /// 后台工作区作业回复前的协议通知（如 `plugins/operationProgress`），按发出顺序转给 Host。
    AuxiliaryNotify {
        method: String,
        params: Value,
    },
    RequestAuth {
        provider: String,
        selection: Value,
        access: Value,
        reply: oneshot::Sender<Value>,
    },
    ContextUsage(Value),
    CompactStarted {
        id: String,
        manual: bool,
        tokens: usize,
        committed: oneshot::Sender<()>,
    },
    CompactDone {
        id: String,
        context: zcode_cli_domain::context::ContextState,
        tokens: usize,
        usage: Value,
        committed: oneshot::Sender<()>,
    },
    Background {
        task: zcode_cli_domain::background::BackgroundTask,
        committed: Option<oneshot::Sender<()>>,
    },
    Retry(Option<RetryState>),
    /// 模型请求的网络状态（TS ModelNetworkStatus：started / completed / failed / retry_scheduled），只进遥测事实。
    ModelStatus(serde_json::Value),
    Text {
        response_id: String,
        text: String,
        reasoning: bool,
    },
    ModelDone {
        response_id: String,
        stable: bool,
        message: Option<Value>,
        usage: Value,
        committed: oneshot::Sender<()>,
    },
    /// 成功轮次收尾的浏览器截图卡（TS `browser_turn_end` 工具行）。
    BrowserTurnScreenshot {
        display: Value,
        committed: oneshot::Sender<()>,
    },
    ToolStart {
        call: Value,
        /// MCP 工具卡的展示元数据（TS `createMcpToolDisplay`，kind=mcp_tool）；非 MCP 为 None。
        display: Option<Value>,
    },
    Permission {
        call: Value,
        /// 主会话启用记忆时的记忆根：写入其中的 Markdown 放行（TS applyMemoryFilePermission）。
        memory_root: Option<String>,
        /// 工具的审批门答复 proceed（TS `prepareApproval`）：判定为 ask 时直接放行，deny 仍然生效。
        approval_proceed: bool,
        /// PreToolUse hook 的 allow / ask（`{behavior, reason}`，TS applyPreToolPermissionDecision）。
        hook: Option<Value>,
        reply: oneshot::Sender<PermissionOutcome>,
    },
    Question {
        call_id: String,
        input: Box<zcode_cli_domain::question::QuestionInput>,
        reply: oneshot::Sender<zcode_cli_domain::question::QuestionAnswer>,
    },
    /// EnterPlanMode / ExitPlanMode：由会话 owner 改变 plan 状态并（退出时）发起审批交互。
    PlanEnter {
        reply: oneshot::Sender<ToolOutput>,
    },
    PlanExit {
        call_id: String,
        input: Value,
        reply: oneshot::Sender<ToolOutput>,
    },
    ToolDone {
        id: String,
        tool: String,
        result: String,
        /// 工具结果媒体（chat 形态 part，含 data URL）；会话 owner 落附件存储后以引用入史。
        media: Vec<Value>,
        display: Option<Value>,
        failed: bool,
        /// 权限被拒（用户或规则）：模型仍收到拒绝文案，但行按 TS 收口为 cancelled、不带输出。
        denied: bool,
        committed: oneshot::Sender<()>,
    },
    StepBoundary {
        committed: oneshot::Sender<Option<Vec<Value>>>,
    },
    Finished {
        error: Option<String>,
        model_failure: Option<ModelFailure>,
        cancelled: bool,
    },
}
pub struct ModelOutput {
    /// 本次（最终成功那次尝试的）模型响应 id；与 Text 事件的 response_id 相同，供工具行 `assistantResponseId`。
    pub response_id: String,
    pub message: Value,
    pub calls: Vec<Value>,
    pub usage: Value,
    pub output_limit: bool,
}
/// 权限判定结果：不允许时带拒绝文案（对应 TS `buildPermissionDeniedContent`），
/// 由工具结果逐字回给模型。
#[derive(Clone, Debug)]
pub struct PermissionOutcome {
    pub allowed: bool,
    pub denial: Option<String>,
}

impl PermissionOutcome {
    pub fn allow() -> Self {
        Self {
            allowed: true,
            denial: None,
        }
    }
    pub fn deny(content: String) -> Self {
        Self {
            allowed: false,
            denial: Some(content),
        }
    }
}

#[derive(Clone)]
pub struct EventSink {
    pub session_id: String,
    pub run_id: String,
    pub tx: mpsc::Sender<RunEvent>,
}
impl EventSink {
    pub async fn send(&self, event: Event) -> Result<()> {
        self.tx
            .send(RunEvent {
                session_id: self.session_id.clone(),
                run_id: self.run_id.clone(),
                event,
            })
            .await?;
        Ok(())
    }
}

/// 工具层的长期 Host 通道（不依附会话）：该 id 上的 `Event::HostRequest` 由 Engine 直接转发给 Host。
/// 见 docs/specs/rust-mcp-official-auth.md「所有者与事件顺序」。
pub const HOST_CHANNEL: &str = "rust-host-channel";

/// Host 反向请求的应答：原始结果 JSON 文本，或 (code, message)。
pub type HostReply = oneshot::Sender<std::result::Result<String, (i64, String)>>;

pub enum Input {
    Request(zcode_cli_domain::protocol::Request),
    /// Host 对 runtime 反向请求的应答；`raw_result` 保留原始 JSON 文本（需要键顺序时重新解析）。
    Response {
        id: String,
        result: Value,
        error: Option<Value>,
        raw_result: Option<String>,
    },
    Invalid,
    TooLarge,
    Eof,
}
