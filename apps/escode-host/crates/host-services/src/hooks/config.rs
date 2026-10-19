//! Workspace hooks 配置 schema 与发现（TS `workspace-hook-config.ts` 的严格子集）。
//!
//! Zod schema 的 strict/passthrough 语义在这里显式实现：根与 matcher strict，hook 声明
//! passthrough（未知键作为 `custom` 原样保留）；声明解析失败的项目源被跳过（与 TS 同）。

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use super::paths;

pub const DEFAULT_TIMEOUT_MS: f64 = 60_000.0;
pub const DEFAULT_MAX_OUTPUT_BYTES: f64 = 32_768.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HookEvent {
    SessionStart,
    UserPromptSubmit,
    PreToolUse,
    PermissionRequest,
    PostToolUse,
    PostToolUseFailure,
    Stop,
}

impl HookEvent {
    pub const ALL: [Self; 7] = [
        Self::SessionStart,
        Self::UserPromptSubmit,
        Self::PreToolUse,
        Self::PermissionRequest,
        Self::PostToolUse,
        Self::PostToolUseFailure,
        Self::Stop,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::SessionStart => "SessionStart",
            Self::UserPromptSubmit => "UserPromptSubmit",
            Self::PreToolUse => "PreToolUse",
            Self::PermissionRequest => "PermissionRequest",
            Self::PostToolUse => "PostToolUse",
            Self::PostToolUseFailure => "PostToolUseFailure",
            Self::Stop => "Stop",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|event| event.as_str() == value)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Shell {
    Enabled,
    Command(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HookType {
    Command,
    Process,
}

impl HookType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Command => "command",
            Self::Process => "process",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct HookDefinition {
    pub kind: HookType,
    pub command: String,
    pub enabled: Option<bool>,
    pub args: Option<Vec<String>>,
    pub async_: Option<bool>,
    pub shell: Option<Shell>,
    pub timeout: Option<f64>,
    pub timeout_ms: Option<f64>,
    pub status_message: Option<String>,
    /// 声明中不在 TS `getCustomHookFields` 解构集合里的键（原值）。
    pub custom: Map<String, Value>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Matcher {
    pub matcher: Option<String>,
    pub hooks: Vec<HookDefinition>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HooksConfig {
    pub enabled: Option<bool>,
    pub timeout_ms: Option<f64>,
    pub max_output_bytes: Option<f64>,
    pub events: [Option<Vec<Matcher>>; 7],
}

#[derive(Clone, Debug)]
pub struct HookSource {
    pub canonical_path: PathBuf,
    pub base_dir: PathBuf,
    pub discovery_order: u64,
    pub config_file_kind: &'static str,
    pub explicit_project_config: bool,
    pub editable: bool,
    pub hooks: HooksConfig,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RuntimeRoot {
    pub enabled: bool,
    pub timeout_ms: f64,
    pub max_output_bytes: f64,
}

impl RuntimeRoot {
    pub fn resolve<'a>(roots: impl IntoIterator<Item = &'a HooksConfig>) -> Self {
        let mut enabled = false;
        let mut timeout_ms = DEFAULT_TIMEOUT_MS;
        let mut max_output_bytes = DEFAULT_MAX_OUTPUT_BYTES;
        for root in roots {
            if root.enabled == Some(true) {
                enabled = true;
            }
            if let Some(value) = root.timeout_ms {
                timeout_ms = value;
            }
            if let Some(value) = root.max_output_bytes {
                max_output_bytes = value;
            }
        }
        Self {
            enabled,
            timeout_ms: rounded_at_least_one(timeout_ms),
            max_output_bytes: rounded_at_least_one(max_output_bytes),
        }
    }
}

pub fn rounded_at_least_one(value: f64) -> f64 {
    value.round().max(1.0)
}

/// `resolveWorkspaceHookConfiguredGates`。
pub fn configured_gates(
    source_enabled: Option<bool>,
    declaration_enabled: Option<bool>,
    runtime_hooks_enabled: bool,
) -> (bool, bool, bool) {
    let source_root_enabled = source_enabled != Some(false);
    let declaration_enabled = declaration_enabled != Some(false);
    (
        source_root_enabled,
        declaration_enabled,
        source_root_enabled && declaration_enabled && runtime_hooks_enabled,
    )
}

/// `resolveWorkspaceHookTimeoutMs` → 取整毫秒。
pub fn resolved_timeout_ms(hook: &HookDefinition, default_timeout_ms: f64) -> u64 {
    let value = match hook.timeout_ms {
        Some(value) => value,
        None if hook.kind == HookType::Command => hook
            .timeout
            .map_or(default_timeout_ms, |seconds| seconds * 1000.0),
        None => default_timeout_ms,
    };
    value.round().max(1.0) as u64
}

pub fn hook_timeout_seconds(hook: &HookDefinition) -> Option<f64> {
    match (hook.kind, hook.timeout) {
        (HookType::Command, Some(seconds)) => Some(seconds),
        _ => hook.timeout_ms.map(|ms| ms.round() / 1000.0),
    }
}

pub async fn read_user_source(home: &Path) -> Option<HookSource> {
    let path = paths::join(home, &[".escode", "cli", "config.json"]);
    let value = read_json_object(&path).await?;
    let hooks = parse_hooks_config(value.get("hooks")?).ok()?;
    let mut source = create_source(&path, home, hooks, 0, true);
    // TS `readUserESCodeSource` 在 explicitProjectConfig 之外显式覆写 `editable: true`。
    source.editable = true;
    Some(source)
}

/// TS `readWorkspaceHookProjectSources().sources`：坏文件/坏 schema/无 `hooks` 的项目源被跳过。
pub async fn read_project_sources(workspace: &Path) -> Vec<HookSource> {
    let files = paths::discover_config_files(workspace).await;
    let mut sources = Vec::new();
    for (order, path) in files.into_iter().enumerate() {
        let Some(value) = read_json_object(&path).await else {
            continue;
        };
        let Some(raw_hooks) = value.get("hooks") else {
            continue;
        };
        let Ok(hooks) = parse_hooks_config(raw_hooks) else {
            continue;
        };
        sources.push(create_source(
            &path,
            workspace,
            hooks,
            u64::try_from(order).unwrap_or(u64::MAX),
            false,
        ));
    }
    sources
}

pub async fn read_json_object(path: &Path) -> Option<Map<String, Value>> {
    let text = tokio::fs::read_to_string(path).await.ok()?;
    match serde_json::from_str::<Value>(&text).ok()? {
        Value::Object(object) => Some(object),
        _ => None,
    }
}

fn create_source(
    path: &Path,
    workspace: &Path,
    hooks: HooksConfig,
    discovery_order: u64,
    explicit: bool,
) -> HookSource {
    let canonical_path = paths::normalize(path);
    let config_directory = paths::parent(&canonical_path);
    let base_dir = if paths::file_name(&config_directory) == ".escode" {
        paths::parent(&config_directory)
    } else {
        config_directory
    };
    let config_file_kind = if explicit {
        "explicit"
    } else if paths::file_name(&canonical_path) == "escode.json" {
        "escode.json"
    } else {
        ".escode/config.json"
    };
    let editable =
        !explicit && canonical_path == paths::join(workspace, &[".escode", "config.json"]);
    HookSource {
        canonical_path,
        base_dir,
        discovery_order,
        config_file_kind,
        explicit_project_config: explicit,
        editable,
        hooks,
    }
}

pub(crate) use super::schema::event_index;
pub use super::schema::parse_hooks_config;
