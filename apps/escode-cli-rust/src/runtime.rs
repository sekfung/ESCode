//! app-server 与 `-p`（docs/specs/rust-headless-prompt.md）共用的运行时装配：工作区与数据目录、TS 历史导入、Engine 构建。
//! 两个入口只在传输与启动握手上不同，执行语义来自同一个 Engine。
use anyhow::{Context, Result};
use escode_cli_core::Engine;
use escode_cli_core_api::{ModelIdentity, ModelPort, ModelRegistry, RuntimePorts};
use escode_cli_host::{SystemClock, WorkspaceContext, legacy_paths};
use escode_cli_model::{config::ModelConfig, provider::HttpModel, registry::Registry};
use escode_cli_state::Store;
use escode_cli_tools::WorkspaceTools;
use std::path::PathBuf;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// 一次运行的工作区事实。
pub struct Workspace {
    /// Host / 用户给出的工作区路径（提示词与身份用它，不 realpath）。
    pub requested_cwd: PathBuf,
    /// realpath 后的工作区（工具执行用）。
    pub cwd: PathBuf,
    pub data_dir: PathBuf,
    /// Rust 会话库。
    pub db: PathBuf,
    /// 会话身份（`ESCODE_WORKSPACE_IDENTITY` 或路径）。
    pub identity: String,
}

pub async fn resolve_workspace(
    cwd: Option<PathBuf>,
    data_dir: Option<PathBuf>,
) -> Result<Workspace> {
    let requested_cwd = cwd.unwrap_or(std::env::current_dir()?);
    let cwd = escode_cli_host::realpath(&requested_cwd)
        .await
        .context("Workspace unavailable")?;
    // 修复：配置文件的 `network` 段（代理 / No Proxy / CA）原先被忽略。必须在任何 HTTP 客户端建立前写入；
    // 读失败只告警、按无文件值继续（TS 文件配置有诊断时同样忽略该文件）。
    match escode_cli_tools::network_file_config(&cwd).await {
        Ok(network) => escode_cli_host::net_config::install(network),
        Err(error) => escode_cli_host::file_log::warn(
            "network.config.ignored",
            "cli",
            &format!("network config ignored: {error}"),
            serde_json::Value::Null,
        ),
    }
    let requested_data = data_dir.unwrap_or_else(|| {
        std::env::var_os("ESCODE_CLI_RUST_DATA_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(
                    std::env::var_os("HOME")
                        .or_else(|| std::env::var_os("USERPROFILE"))
                        .unwrap_or_default(),
                )
                .join(".escode")
                .join("rust")
            })
    });
    let data_dir = if requested_data.is_absolute() {
        requested_data
    } else {
        std::env::current_dir()?.join(requested_data)
    };
    tokio::fs::create_dir_all(&data_dir).await?;
    let data_dir = escode_cli_host::realpath(data_dir).await?;
    let db = data_dir.join("rust-sessions.sqlite");
    // 身份使用 Host 提交的路径，不把 macOS /var -> /private/var 的 realpath 改写成新工作区。
    let identity = escode_cli_host::workspace_identity(
        std::env::var("ESCODE_WORKSPACE_IDENTITY").ok().as_deref(),
        &requested_cwd,
    );
    Ok(Workspace {
        requested_cwd,
        cwd,
        data_dir,
        db,
        identity,
    })
}

/// 按需导入 TS 历史（数据迁移）。`stop` 触发时取消导入并返回 `Ok(true)`，调用方应直接退出。
pub async fn import_history(
    store: &Store,
    ws: &Workspace,
    explicit: Option<PathBuf>,
    allow_implicit: bool,
    stop: &CancellationToken,
) -> Result<bool> {
    let Some(source) = legacy_paths::resolve(explicit, &ws.requested_cwd, allow_implicit).await?
    else {
        return Ok(false);
    };
    if !tokio::fs::try_exists(&source.database).await? {
        anyhow::ensure!(!source.required, "Explicit TS import source does not exist");
        return Ok(false);
    }
    let import_cancel = stop.child_token();
    let operation = store.import_ts(
        source.database,
        ws.identity.clone(),
        ws.requested_cwd.to_string_lossy().into_owned(),
        ws.data_dir.clone(),
        source.artifacts,
        import_cancel.clone(),
    );
    tokio::pin!(operation);
    // 只在实际导入期间处理停止；无导入时保留输入缓冲区交由 actor 排空。
    let result = tokio::select! {
        result = &mut operation => result,
        _ = stop.cancelled() => {
            import_cancel.cancel();
            operation.await
        }
    };
    // 只有实际执行过导入时，停止信号（含导入期间 Host 关闭输入）才让调用方退出；无导入时保留输入缓冲区交由 actor 排空。
    if import_cancel.is_cancelled() || stop.is_cancelled() {
        return Ok(true);
    }
    result?;
    Ok(false)
}

/// 构建 Engine：模型（`--config` 或 Registry 环境变量）、工作区上下文、存储与工具端口。
pub async fn build_engine(
    ws: &Workspace,
    store: Store,
    config: Option<&PathBuf>,
    desktop: bool,
) -> Result<Engine> {
    // TS 派生媒体缓存位于 `<storageRoot>/cli/{image,pdf,video}-cache`（docs/specs/rust-media-read.md 第 4 期）。
    if let Ok(root) = legacy_paths::storage_root(&ws.requested_cwd).await {
        escode_cli_model::set_media_cache_root(root.join("cli"));
    }
    let question_timing = escode_cli_host::question_timing()?;
    let model_config = ModelConfig::load(config).await?;
    let registry = if model_config.is_none() {
        Registry::from_env()
            .await?
            .map(|r| r as Arc<dyn ModelRegistry>)
    } else {
        None
    };
    let identity = model_config.as_ref().map(|c| ModelIdentity {
        provider_id: c.provider_id.clone(),
        model_id: c.model_id.clone(),
        reasoning_level: c.reasoning_level.clone(),
    });
    let model = model_config
        .map(HttpModel::new)
        .map(|m| Arc::new(m) as Arc<dyn ModelPort>);
    let workspace_path = std::path::absolute(&ws.requested_cwd).unwrap_or_else(|_| ws.cwd.clone());
    Ok(Engine::new(
        ws.identity.clone(),
        identity,
        RuntimePorts {
            // 修复：提示词里的工作目录与 AGENTS/Git 查找用 Host 提交的路径（TS 同样不 realpath）；
            // 原先用 realpath，macOS /var→/private/var、Windows 8.3 短名会与 Node 不一致。
            context: Arc::new(WorkspaceContext::new(
                workspace_path.clone(),
                std::env::var_os("HOME")
                    .filter(|s| !s.is_empty())
                    .or_else(|| std::env::var_os("USERPROFILE"))
                    .map(PathBuf::from)
                    .unwrap_or_default(),
                desktop,
            )),
            store: Arc::new(store),
            model,
            tools: Arc::new(
                WorkspaceTools::new(ws.cwd.clone(), ws.data_dir.join("tool-results"))
                    .with_workspace_path(workspace_path)
                    .with_session_db(ws.db.clone())
                    .with_model_admission(),
            ),
            clock: Arc::new(SystemClock),
        },
    )
    .await?
    .with_question_timing(question_timing.0, question_timing.1)
    .with_registry(registry, ws.requested_cwd.to_string_lossy().into_owned()))
}

/// 模型是否已配置（`-p` 在建 Engine 前给出明确错误）。
pub fn model_configured(config: Option<&PathBuf>) -> bool {
    config.is_some()
        || (std::env::var_os("ESCODE_BUILTIN_PROVIDER_CONFIG_FILE").is_some()
            && std::env::var_os("ESCODE_PERSONAL_PROVIDER_CONFIG_FILE").is_some())
}
