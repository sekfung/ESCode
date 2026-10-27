//! ESCode App 级 Rust Host 进程入口（docs/specs/rust-u2-host-skeleton.md §3.4/§3.5）。
//!
//! 职责（AGENTS.md 状态所有者：Host 拥有实例表、代次、订阅归属与调度）：
//! - Main 经 stdio 控制通道提交 `WindowAttach`（含 workspace 描述），Host 为该 workspace
//!   建/复用 runtime 实例（`host-runtime::Instances`：single-flight + 懒重启 generation）；
//! - Main 转发的 Renderer 请求行进入实例，实例出站帧按 `WindowFrame` 回给 Main；
//! - `Shutdown`：逐个实例 cancel → 宽限 drain → 残余清单 + 退出码。
//!
//! 本进程不承载业务状态，也不监听信号以外的事件源；RuntimeOptions 由控制消息提供，
//! 进程 env 只用于自身日志与 HOME 缺省（组合根职责，类似 CLI 的 main.rs）。

mod control;

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use escode_host_control::{
    ControlError, ControlErrorCode, ControlMessage, ControlWriter, WorkspaceDescriptor,
};
use escode_host_runtime::{Instance, Instances, Link};
use serde_json::{Value, json};

use crate::control::ControlLoop;

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis() as u64)
}

/// 协议版本（Main 握手用；不匹配时 Main 应回退 Node Host）。
const PROTOCOL_VERSION: u32 = 1;

#[tokio::main]
async fn main() {
    let code = match run().await {
        Ok(code) => code,
        Err(error) => {
            // 启动期错误用结构化控制帧上抛，Main 据此决定重试或回退（不静默退出）。
            eprintln!("escode-host: {error:#}");
            1
        }
    };
    std::process::exit(code);
}

async fn run() -> Result<i32> {
    escode_cli_core_api::install_process_snapshot();
    let link = link_from_env();
    let instances = Instances::new(link);
    let mut control = ControlLoop::start().context("Host control channel unavailable")?;
    // 服务域（host-rpc / host-services）经进程级 sink 记日志：与控制消息共用一条有序队列，
    // 由 Main 消费 `logBatch` 帧（docs/specs/rust-u2-host-skeleton.md §3.2/§3.5）。
    escode_host_control::logging::install_sink(control.log_sink());
    control
        .send(ControlMessage::HostReady {
            protocol_version: PROTOCOL_VERSION,
            workspaces: 0,
        })
        .await?;
    let code = serve(&mut control, &instances).await?;
    // 收口出站队列：等泵把已入队的日志与 `stopped` 写完再退出。
    control.close().await;
    Ok(code)
}

/// runtime-link 选择器：默认 embedded；`ESCODE_HOST_RUNTIME_LINK=child` 时用 CLI 子进程
/// （差分、回退与故障隔离选项）；`ESCODE_HOST_RUNTIME_BINARY` 指定二进制路径。
fn link_from_env() -> Link {
    let child = std::env::var("ESCODE_HOST_RUNTIME_LINK").is_ok_and(|value| value == "child");
    if !child {
        return Link::Embedded;
    }
    let binary = std::env::var("ESCODE_HOST_RUNTIME_BINARY").map_or_else(
        |_| {
            std::env::current_exe()
                .ok()
                .and_then(|exe| exe.parent().map(|dir| dir.join("escode-cli-rust")))
                .unwrap_or_default()
        },
        std::path::PathBuf::from,
    );
    Link::Child { binary }
}

/// 为一个 workspace 建/复用实例；窗口级 env patch 只在该实例首次创建时生效。
async fn prepare_workspace(
    instances: &Instances,
    identity: &str,
    workspace: &WorkspaceDescriptor,
    runtime_env: &BTreeMap<String, String>,
    patches: &Arc<Mutex<HashMap<String, BTreeMap<String, String>>>>,
) -> Result<Arc<Instance>> {
    let patch = patches
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(identity)
        .cloned();
    instances
        .get(identity, || {
            control::runtime_options_with_patch(workspace, runtime_env, patch.as_ref())
        })
        .await
}

/// 把实例的真实 `startup/storageState` 逐条转给 Main，直到 ready/failed 或实例关闭。
fn forward_startup(
    writer: ControlWriter,
    instance: Arc<Instance>,
    startup_id: String,
    window_id: String,
    workspace_key: String,
) {
    let generation = instance.generation;
    let mut startup = instance.subscribe_startup();
    tokio::spawn(async move {
        loop {
            let snapshot = startup.borrow_and_update().clone();
            if let Some(state) = snapshot {
                let phase = state["phase"].as_str().unwrap_or_default();
                let terminal = phase == "ready" || phase == "failed";
                let sent = writer
                    .control(ControlMessage::WorkspaceStartupState {
                        startup_id: startup_id.clone(),
                        window_id: window_id.clone(),
                        workspace_key: workspace_key.clone(),
                        generation,
                        state,
                    })
                    .await;
                if sent.is_err() || terminal {
                    return;
                }
            }
            if startup.changed().await.is_err() {
                return;
            }
        }
    });
}

/// 控制循环：attach/detach/请求转发/关闭。
/// 停止持有控制写句柄的后台转发任务，并给它们一个短窗口自行退出。
///
/// 这些任务不是控制队列的所有者，但它们的克隆会让 `ControlLoop::close` 等满 2s 超时；
/// 显式取消后泵能立即看到队列关闭（退出路径实测从 ~2s 降到几十毫秒）。
async fn stop_background_tasks(
    token: &tokio_util::sync::CancellationToken,
    tasks: &mut Vec<tokio::task::JoinHandle<()>>,
) {
    token.cancel();
    for mut task in tasks.drain(..) {
        if tokio::time::timeout(std::time::Duration::from_millis(500), &mut task)
            .await
            .is_err()
        {
            task.abort();
        }
    }
}

async fn serve(control: &mut ControlLoop, instances: &Instances) -> Result<i32> {
    // window → 实例身份（attachment 路由由 Host 拥有）。
    let mut attachments: BTreeMap<String, String> = BTreeMap::new();
    // Renderer 传输面（hello/NDJSON 版，供早期接入联调）。
    let mut surfaces: Vec<escode_host_surface::Surface> = Vec::new();
    // Renderer RPC 面（@escode/rpc 协议，Renderer 真实链路）。
    let mut rpc_surfaces: Vec<escode_host_rpc::RpcSurface> = Vec::new();
    // 本地媒体路径授权 broker（Main 声明 `localMediaPreview` 才有；持有控制队列写句柄，收口时必须 close）。
    let mut path_authorizer: Option<
        std::sync::Arc<escode_host_control::path_authorizer::PathAuthorizer>,
    > = None;
    // Main 专属服务 broker（cua-permission / sandbox 注册）。
    let mut main_broker: Option<std::sync::Arc<escode_host_control::main_broker::MainBroker>> =
        None;
    // `rpcListen.runtimeEnv` 的 Host 级快照；WorkspacePrepare 的窗口级 patch 按 identity 覆盖。
    let mut host_runtime_env: BTreeMap<String, String> = BTreeMap::new();
    let patches: Arc<Mutex<HashMap<String, BTreeMap<String, String>>>> = Arc::default();
    let sink_writer = control.writer_handle();
    escode_host_rpc::host_events::set_sink(Arc::new(move |event| {
        let writer = sink_writer.clone();
        tokio::spawn(async move {
            let _ = writer.control(ControlMessage::HostEvent { event }).await;
        });
    }));
    // 后台转发任务共用收口信号：它们都持有控制队列写句柄，若不显式退出，
    // `ControlLoop::close` 会等满 2s 超时再 abort（实测退出总耗时的主要来源）。
    let background_stop = tokio_util::sync::CancellationToken::new();
    let mut background_tasks: Vec<tokio::task::JoinHandle<()>> = Vec::new();
    // 实例生命周期 → Main 的 agent 进程事件 + CUA 操作状态。
    let events_writer = control.writer_handle();
    let lifecycle_instances = instances.clone();
    let mut lifecycle = instances.lifecycle();
    let lifecycle_stop = background_stop.clone();
    background_tasks.push(tokio::spawn(async move {
        loop {
            let event = tokio::select! {
                _ = lifecycle_stop.cancelled() => return,
                event = lifecycle.recv() => match event {
                    Some(event) => event,
                    None => return,
                },
            };
            match event.state {
                escode_host_runtime::RuntimeState::Available => {
                    if let Some(pid) = event.process_id {
                        let _ = events_writer
                            .control(ControlMessage::HostEvent {
                                event: json!({
                                    "type": "agent-process-spawned",
                                    "pid": pid,
                                    "provider": Value::Null,
                                    "workspacePath": event.workspace_path,
                                    "command": "escode-cli-rust",
                                    "args": ["app-server", "--stdio"],
                                    "startedAt": now_ms(),
                                    "runtimeGeneration": event.generation,
                                    "runtimeInstanceId": event.runtime_identity,
                                }),
                            })
                            .await;
                    }
                    if let Some(instance) = lifecycle_instances.live(&event.identity) {
                        let writer = events_writer.clone();
                        let path = instance.workspace_path().to_owned();
                        let identity = instance.workspace_identity().map(str::to_owned);
                        tokio::spawn(async move {
                            let mut frames = instance.subscribe().await;
                            while let Some(line) = frames.recv().await {
                                let Ok(message) = serde_json::from_str::<Value>(&line) else {
                                    continue;
                                };
                                if message["method"] != "computer-use/operation-event" {
                                    continue;
                                }
                                let params = &message["params"];
                                let kind = params["kind"].as_str().unwrap_or_default();
                                let active = kind == "tool-started";
                                if !active && !matches!(kind, "turn-completed" | "turn-failed") {
                                    continue;
                                }
                                let mut value = json!({
                                    "type": "cua-operation-state",
                                    "active": active,
                                    "sessionId": params["sessionId"],
                                    "turnId": params["turnId"],
                                    "workspacePath": path,
                                });
                                if let Some(identity) = &identity {
                                    value["workspaceIdentity"] = Value::from(identity.clone());
                                }
                                let _ = writer
                                    .control(ControlMessage::HostEvent { event: value })
                                    .await;
                            }
                        });
                    }
                }
                escode_host_runtime::RuntimeState::Unavailable => {
                    if let Some(pid) = event.process_id {
                        let _ = events_writer
                            .control(ControlMessage::HostEvent {
                                event: json!({
                                    "type": "agent-process-exited",
                                    "pid": pid,
                                    "runtimeGeneration": event.generation,
                                    "runtimeInstanceId": event.runtime_identity,
                                }),
                            })
                            .await;
                    }
                }
            }
        }
    }));
    let capture_writer = control.writer_handle();
    let capture_stop = background_stop.clone();
    background_tasks.push(tokio::spawn(async move {
        loop {
            let cancelled = tokio::select! {
                _ = capture_stop.cancelled() => true,
                _ = tokio::time::sleep(std::time::Duration::from_millis(250)) => false,
            };
            // 收口时再刷一次：最后一小批网络捕获不能因为取消而丢。
            if let Some(event) = escode_cli_net::capture::flush() {
                let _ = capture_writer
                    .control(ControlMessage::HostEvent { event })
                    .await;
            }
            if cancelled {
                return;
            }
        }
    }));
    while let Some(message) = control.recv().await {
        match message {
            ControlMessage::WorkspacePrepare {
                startup_id,
                window_id,
                workspace,
                runtime_env_patch,
            } => {
                let identity = control::identity_of(&workspace);
                if let Some(patch) = runtime_env_patch {
                    patches
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .insert(identity.clone(), patch);
                }
                match prepare_workspace(
                    instances,
                    &identity,
                    &workspace,
                    &host_runtime_env,
                    &patches,
                )
                .await
                {
                    Ok(instance) => {
                        forward_startup(
                            control.writer_handle(),
                            instance,
                            startup_id,
                            window_id,
                            identity,
                        );
                    }
                    Err(error) => {
                        control
                            .send(ControlMessage::Failed {
                                error: ControlError {
                                    code: ControlErrorCode::Fatal,
                                    message: format!("{error:#}"),
                                },
                            })
                            .await?;
                    }
                }
            }
            ControlMessage::WorkspaceStartupRetry {
                startup_id,
                window_id,
                workspace,
            } => {
                let identity = control::identity_of(&workspace);
                let _ = instances.close(&identity).await;
                match prepare_workspace(
                    instances,
                    &identity,
                    &workspace,
                    &host_runtime_env,
                    &patches,
                )
                .await
                {
                    Ok(instance) => {
                        forward_startup(
                            control.writer_handle(),
                            instance,
                            startup_id,
                            window_id,
                            identity,
                        );
                    }
                    Err(error) => {
                        control
                            .send(ControlMessage::Failed {
                                error: ControlError {
                                    code: ControlErrorCode::Fatal,
                                    message: format!("{error:#}"),
                                },
                            })
                            .await?;
                    }
                }
            }
            ControlMessage::WindowAttach {
                window_id,
                client_id: _,
                workspace,
            } => {
                let identity = control::identity_of(&workspace);
                match instances
                    .get(&identity, || control::runtime_options(&workspace))
                    .await
                {
                    Ok(instance) => {
                        attachments.insert(window_id.clone(), identity.clone());
                        control
                            .send(ControlMessage::WindowAttached {
                                window_id: window_id.clone(),
                                generation: instance.generation,
                            })
                            .await?;
                        // 帧泵按实例一个，向全部 attachment 扇出（多窗口共享同一实例）。
                        control.attach(&identity, &window_id, instance);
                    }
                    Err(error) => {
                        let code = if error
                            .downcast_ref::<escode_cli_rust::adapters::storage::WorkspaceOwned>()
                            .is_some()
                        {
                            ControlErrorCode::WorkspaceOwned
                        } else {
                            ControlErrorCode::Fatal
                        };
                        control
                            .send(ControlMessage::Failed {
                                error: ControlError {
                                    code,
                                    message: format!("{error:#}"),
                                },
                            })
                            .await?;
                    }
                }
            }
            ControlMessage::WindowDetach { window_id } => {
                if let Some(identity) = attachments.remove(&window_id) {
                    control.detach(&identity, &window_id);
                }
                control
                    .send(ControlMessage::WindowDetached { window_id })
                    .await?;
            }
            ControlMessage::WindowRequest { window_id, line } => {
                let Some(identity) = attachments.get(&window_id).cloned() else {
                    control
                        .send(ControlMessage::Failed {
                            error: ControlError {
                                code: ControlErrorCode::NotReady,
                                message: format!("window {window_id} is not attached"),
                            },
                        })
                        .await?;
                    continue;
                };
                let instance = instances
                    .live(&identity)
                    .with_context(|| format!("workspace instance for {identity} is not available"));
                match instance {
                    Ok(instance) => {
                        if let Err(error) = instance.send_line(&line).await {
                            control
                                .send(ControlMessage::Failed {
                                    error: ControlError {
                                        code: ControlErrorCode::Fatal,
                                        message: format!("{error:#}"),
                                    },
                                })
                                .await?;
                        }
                    }
                    Err(error) => {
                        control
                            .send(ControlMessage::Failed {
                                error: ControlError {
                                    code: ControlErrorCode::NotReady,
                                    message: format!("{error:#}"),
                                },
                            })
                            .await?;
                    }
                }
            }
            ControlMessage::RpcListen {
                bind,
                home,
                data_base_dir,
                runtime_env,
                local_media_preview,
                main_services,
            } => {
                let addr: std::net::SocketAddr = bind
                    .parse()
                    .unwrap_or_else(|_| std::net::SocketAddr::from(([127, 0, 0, 1], 0)));
                let runtime_env = runtime_env.unwrap_or_default();
                host_runtime_env = runtime_env.clone();
                // provider 配置文件路径：Main 合成的运行时环境里的两个约定变量（A16b 的 P1 部分）。
                let provider_files = escode_host_services::provider::ProviderFiles::from_env(
                    |name| runtime_env.get(name).map(String::as_str),
                    Some(std::path::Path::new(&home)),
                );
                if local_media_preview && let Some(previous) = path_authorizer.take() {
                    previous.close();
                }
                let authorizer = local_media_preview.then(|| {
                    escode_host_control::path_authorizer::PathAuthorizer::new(
                        control.writer_handle(),
                    )
                });
                path_authorizer.clone_from(&authorizer);
                if main_services && let Some(previous) = main_broker.take() {
                    previous.close();
                }
                let broker = main_services.then(|| {
                    escode_host_control::main_broker::MainBroker::new(control.writer_handle())
                });
                main_broker.clone_from(&broker);
                let client_services = Some(std::sync::Arc::new(
                    escode_host_services::client_services::ClientServices::production(
                        |name| runtime_env.get(name).map(String::as_str),
                        Some(std::path::Path::new(&home)),
                    ),
                ));
                let env_for_options = runtime_env.clone();
                let patches_for_options = Arc::clone(&patches);
                let options: escode_host_surface::OptionsFactory =
                    std::sync::Arc::new(move |workspace| {
                        let patch = patches_for_options
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .get(&control::identity_of(workspace))
                            .cloned();
                        control::runtime_options_with_patch(
                            workspace,
                            &env_for_options,
                            patch.as_ref(),
                        )
                    });
                let channels = escode_host_rpc::domains::channels_with(
                    std::path::PathBuf::from(home),
                    data_base_dir.map(std::path::PathBuf::from),
                    instances.clone(),
                    options,
                    provider_files,
                    authorizer,
                    broker,
                    client_services,
                );
                match escode_host_rpc::serve(addr, channels, Some(instances.clone())).await {
                    Ok(surface) => {
                        control
                            .send(ControlMessage::RpcListening {
                                addr: surface.addr.to_string(),
                                secret: surface.secret.clone(),
                            })
                            .await?;
                        rpc_surfaces.push(surface);
                    }
                    Err(error) => {
                        control
                            .send(ControlMessage::Failed {
                                error: ControlError {
                                    code: ControlErrorCode::Fatal,
                                    message: format!("rpc listen failed: {error:#}"),
                                },
                            })
                            .await?;
                    }
                }
            }
            ControlMessage::SurfaceListen { bind } => {
                let addr: std::net::SocketAddr = bind
                    .parse()
                    .unwrap_or_else(|_| std::net::SocketAddr::from(([127, 0, 0, 1], 0)));
                let env_for_options = host_runtime_env.clone();
                let patches_for_options = Arc::clone(&patches);
                let options: escode_host_surface::OptionsFactory =
                    std::sync::Arc::new(move |workspace| {
                        let patch = patches_for_options
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .get(&control::identity_of(workspace))
                            .cloned();
                        control::runtime_options_with_patch(
                            workspace,
                            &env_for_options,
                            patch.as_ref(),
                        )
                    });
                match escode_host_surface::Surface::start(addr, instances.clone(), options).await {
                    Ok(surface) => {
                        control
                            .send(ControlMessage::SurfaceListening {
                                addr: surface.addr.to_string(),
                            })
                            .await?;
                        // 传输面随 Host 进程存活；句柄留在这里以便 Shutdown 时收口。
                        surfaces.push(surface);
                    }
                    Err(error) => {
                        control
                            .send(ControlMessage::Failed {
                                error: ControlError {
                                    code: ControlErrorCode::Fatal,
                                    message: format!("surface listen failed: {error:#}"),
                                },
                            })
                            .await?;
                    }
                }
            }
            ControlMessage::SettingsRead { home } => {
                let settings = escode_host_services::settings::read(std::path::Path::new(&home))
                    .await
                    .unwrap_or_else(|error| {
                        eprintln!("escode-host: settings read failed: {error:#}");
                        serde_json::json!({})
                    });
                control
                    .send(ControlMessage::SettingsValue { settings })
                    .await?;
            }
            ControlMessage::SettingsWrite { home, patch } => {
                match escode_host_services::settings::apply_patch(
                    std::path::Path::new(&home),
                    &patch,
                )
                .await
                {
                    Ok(settings) => {
                        control
                            .send(ControlMessage::SettingsValue { settings })
                            .await?;
                    }
                    Err(error) => {
                        control
                            .send(ControlMessage::Failed {
                                error: ControlError {
                                    code: ControlErrorCode::Fatal,
                                    message: format!("{error:#}"),
                                },
                            })
                            .await?;
                    }
                }
            }
            ControlMessage::LocalMediaPreviewAuthorized {
                request_id,
                ok,
                path,
                error,
            } => {
                // 未知/重复的 requestId 丢弃（stale 防护）；没有 broker 时也不应收到。
                let paired = path_authorizer
                    .as_ref()
                    .is_some_and(|broker| broker.resolve(&request_id, ok, path, error));
                if !paired {
                    escode_host_control::logging::warn(
                        "media-preview",
                        format!("dropped unpaired authorization reply {request_id}"),
                    );
                }
            }
            ControlMessage::MainResponse {
                request_id,
                ok,
                result,
                error,
            } => {
                let paired = main_broker
                    .as_ref()
                    .is_some_and(|broker| broker.resolve(&request_id, ok, result, error));
                if !paired {
                    escode_host_control::logging::warn(
                        "main-services",
                        format!("dropped unpaired main service reply {request_id}"),
                    );
                }
            }
            ControlMessage::Shutdown { reason } => {
                if let Some(broker) = path_authorizer.take() {
                    broker.close();
                }
                if let Some(broker) = main_broker.take() {
                    broker.close();
                }
                for surface in surfaces.drain(..) {
                    surface.shutdown().await;
                }
                for surface in rpc_surfaces.drain(..) {
                    surface.shutdown().await;
                }
                control.stop_pumps();
                let _ = control
                    .send(ControlMessage::LogBatch {
                        scope: "host".to_owned(),
                        level: escode_host_control::LogLevel::Info,
                        lines: vec![format!("shutdown: {reason}")],
                    })
                    .await;
                let residual = instances.shutdown().await;
                stop_background_tasks(&background_stop, &mut background_tasks).await;
                control
                    .send(ControlMessage::Stopped {
                        residual: residual.clone(),
                    })
                    .await?;
                // 残余任务/进程在宽限内没收回：非零退出，Main 记录并（阶段 3）回退。
                return Ok(if residual.is_empty() { 0 } else { 2 });
            }
            ControlMessage::ResourceUsageSnapshotRequest { request_id } => {
                let event = escode_host_services::process_sampler::snapshot_event(&request_id);
                control.send(ControlMessage::HostEvent { event }).await?;
            }
            ControlMessage::ResourceUsageSnapshotCancel { request_id: _ } => {}
            ControlMessage::NetworkCapture { capture_id } => {
                escode_cli_net::capture::set_capture_id(capture_id);
            }
            // Host→Main 的消息不应出现在入站方向；忽略以免回环。
            ControlMessage::HostReady { .. }
            | ControlMessage::WindowAttached { .. }
            | ControlMessage::WindowDetached { .. }
            | ControlMessage::WindowFrame { .. }
            | ControlMessage::LogBatch { .. }
            | ControlMessage::Failed { .. }
            | ControlMessage::Stopped { .. }
            | ControlMessage::SettingsValue { .. }
            | ControlMessage::SurfaceListening { .. }
            | ControlMessage::RpcListening { .. }
            | ControlMessage::LocalMediaPreviewAuthorize { .. }
            | ControlMessage::HostEvent { .. }
            | ControlMessage::WorkspaceStartupState { .. }
            | ControlMessage::MainRequest { .. } => {}
        }
    }
    // 输入关闭（Main 退出）= 收口全部实例与传输面。
    // 逐阶段计时：退出总耗时是阶段 4 的指标，Host 内部不保留其他可观测点。
    let eof_started = std::time::Instant::now();
    if let Some(broker) = path_authorizer.take() {
        broker.close();
    }
    for surface in surfaces.drain(..) {
        surface.shutdown().await;
    }
    for surface in rpc_surfaces.drain(..) {
        surface.shutdown().await;
    }
    let surfaces_ms = eof_started.elapsed().as_millis();
    control.stop_pumps();
    let instances_started = std::time::Instant::now();
    let residual = instances.shutdown().await;
    let instances_ms = instances_started.elapsed().as_millis();
    stop_background_tasks(&background_stop, &mut background_tasks).await;
    escode_host_control::logging::info(
        "host",
        format!(
            "eof shutdown: surfaces={surfaces_ms}ms instances={instances_ms}ms residual={}",
            residual.len()
        ),
    );
    if residual.is_empty() {
        Ok(0)
    } else {
        eprintln!("escode-host: residual work at EOF: {}", residual.join(", "));
        Ok(2)
    }
}
