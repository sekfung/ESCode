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
    ToolStart {
        call: Value,
    },
    Permission {
        call: Value,
        /// 主会话启用记忆时的记忆根：写入其中的 Markdown 放行（TS applyMemoryFilePermission）。
        memory_root: Option<String>,
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
