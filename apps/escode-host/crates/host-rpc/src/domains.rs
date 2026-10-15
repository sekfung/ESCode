//! 已迁移的服务域：channels 表在这里登记（rust-u2-host-skeleton.md §5 优先序）。
//! 未登记的 channel 由 `session::respond` 回显式 `NotImplemented`。

use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

use crate::deferred_drafts::DeferredDrafts;
use crate::domains_session_write::SessionWriteContext;
use crate::domains_task_support::{LazyIndex, ReadContext, RuntimeAccess};
use crate::forward::{Extract, Shaped, runtime_call, runtime_call_shaped, runtime_call_with};
use crate::session::{Channel, Channels, handler};
use crate::task_targets::TaskTargets;
use crate::wire::Wire;

/// `settingService`（TS `ServiceChannels.Setting` = `"setting"`）：
/// `get` 读设置文件、`update` 顶层合并补丁后原子写（与 Node Host 同文件同格式）；
/// 其余方法（`updateDataBaseDir` / `ensureDefaultProject`）未实现——方法表即分派，
/// 缺口由 `respond` 回显式 `NotImplemented` 并列出已实现方法。
pub fn setting_channel(home: std::path::PathBuf) -> Channel {
    let read_home = home.clone();
    let write_home = home;
    Channel::new(
        "setting",
        vec![
            (
                "get",
                handler(move |_arg: Wire| {
                    let home = read_home.clone();
                    Box::pin(async move {
                        escode_host_services::settings::read(&home)
                            .await
                            .map(Wire::Object)
                            .map_err(|error| ("Fatal".to_owned(), format!("{error:#}")))
                    })
                }),
            ),
            (
                "update",
                handler(move |arg: Wire| {
                    let home = write_home.clone();
                    Box::pin(async move {
                        // `ProxyChannel.toService` 把实参打包成数组：`update(patch)` → `[patch]`。
                        let patch = arg
                            .into_args()
                            .into_iter()
                            .next()
                            .and_then(Wire::into_object)
                            .ok_or_else(|| {
                                (
                                    "InvalidRequest".to_owned(),
                                    "setting.update requires a patch object".to_owned(),
                                )
                            })?;
                        escode_host_services::settings::apply_patch(&home, &patch)
                            .await
                            .map(|_| Wire::Undefined)
                            .map_err(|error| ("Fatal".to_owned(), format!("{error:#}")))
                    })
                }),
            ),
        ],
    )
}

/// `escode-session`（TS `ServiceChannels.ESCodeSession` = `"escode-session"`）：
/// 转发按 TS `escodeAgentService.ts` 的请求字面量整形（`session_forward_params`，评审 A11），
/// 带 Host 职责的方法（草稿/索引/thoughtLevel 重放/`withApiRetryRuntime`）见 `domains_session_write.rs`。
/// 未迁移：`readSessionEvents`（运行时无 `session/events`）与 workspace/provider 握手类方法（属各自域）。
pub(crate) fn session_channel(
    instances: escode_host_runtime::Instances,
    options: escode_host_surface::OptionsFactory,
    write: SessionWriteContext,
) -> Channel {
    use crate::domains_session_write as w;

    fn params_of(arg: Wire) -> Value {
        arg.into_args()
            .into_iter()
            .next()
            .and_then(Wire::into_object)
            .unwrap_or(Value::Null)
    }

    // 只做白名单整形与结果抽取的转发。
    let forward = |method: &'static str, with_workspace: bool, extract: Extract| {
        let instances = instances.clone();
        let options = Arc::clone(&options);
        crate::session::handler_rpc(move |arg: Wire| {
            let instances = instances.clone();
            let options = Arc::clone(&options);
            Box::pin(async move {
                let params = params_of(arg);
                let (runtime_method, body) =
                    escode_host_services::session_params::session_forward_params(method, &params)
                        .ok_or_else(|| {
                        crate::session::RpcError::new(
                            "Fatal",
                            format!("no runtime mapping for {method}"),
                        )
                    })?;
                let shaped = Shaped {
                    method: runtime_method,
                    body,
                    with_workspace,
                    trace: None,
                };
                runtime_call_shaped(&instances, &options, params, shaped, extract).await
            })
        })
    };

    // 写索引的方法需要 dataBaseDir：缺省时回显式缺口，不静默成功。
    let indexed = |build: fn(SessionWriteContext) -> crate::session::MethodHandler| {
        if write.read.is_some() {
            build(write.clone())
        } else {
            handler(|_arg: Wire| {
                Box::pin(async move {
                    Err((
                        "NotImplemented".to_owned(),
                        "escode-session writes need dataBaseDir: Main must pass dataBaseDir in rpcListen."
                            .to_owned(),
                    ))
                })
            })
        }
    };

    Channel::new(
        "escode-session",
        vec![
            ("createSession", indexed(w::create_session_handler)),
            ("resumeSession", indexed(w::resume_session_handler)),
            ("setModel", indexed(w::set_model_handler)),
            (
                "listSessions",
                forward("listSessions", true, Extract::Field("sessions")),
            ),
            (
                "readSession",
                w::api_retry_forward_handler(write.clone(), "readSession"),
            ),
            (
                "readSessionMessages",
                forward("readSessionMessages", false, Extract::Field("messages")),
            ),
            ("closeSession", w::close_session_handler(write.clone())),
            (
                "closeDeferredDraftSession",
                w::close_deferred_draft_handler(write.clone()),
            ),
            (
                "promoteDeferredDraftSession",
                w::promote_deferred_draft_handler(write.clone()),
            ),
            (
                "setThoughtLevel",
                w::api_retry_forward_handler(write.clone(), "setThoughtLevel"),
            ),
            (
                "setMode",
                w::api_retry_forward_handler(write.clone(), "setMode"),
            ),
            (
                "initializeWorkspace",
                handler({
                    let instances = instances.clone();
                    let options = Arc::clone(&options);
                    move |arg: Wire| {
                        let instances = instances.clone();
                        let options = Arc::clone(&options);
                        Box::pin(async move {
                            // workspace 预热：TS `agentService.initialize` 返回 agent 可用性与协议常量，
                            // 首屏不因 agent 缺失崩溃（失败也回 `available:false`）。Rust Host 同构：
                            // 建/复用实例后等运行时报到；`transportKind` 固定为 "stdio"。
                            let target = crate::forward::runtime_target(params_of(arg))?;
                            let workspace_key = target.identity.clone();
                            let available = match instances
                                .get(&target.identity, || options(&target.descriptor))
                                .await
                            {
                                Ok(instance) => {
                                    instance
                                        .wait_ready(std::time::Duration::from_secs(30))
                                        .await
                                }
                                Err(_) => false,
                            };
                            Ok(Wire::Object(json!({
                                "available": available,
                                "workspaceKey": workspace_key,
                                "protocolName": "ESCode Protocol",
                                "protocolVersion": 1,
                                "transportKind": "stdio",
                            })))
                        })
                    }
                }),
            ),
            (
                "readWorkspacePresentation",
                handler({
                    let instances = instances.clone();
                    let options = Arc::clone(&options);
                    move |arg: Wire| {
                        let instances = instances.clone();
                        let options = Arc::clone(&options);
                        Box::pin(async move {
                            // workspace 展示面：TS agent 层先读 `runtime/capabilities` 再带
                            // `includeExecutionCapabilities` 读 `workspace/readPresentation`，这里同序转发。
                            // 已登记差异：TS 读取前会同步账号 provider 配置（providers/account 域未迁移）。
                            let params = params_of(arg);
                            let capabilities = runtime_call(
                                &instances,
                                &options,
                                "runtime/capabilities",
                                params.clone(),
                                Extract::Result,
                            )
                            .await
                            .map_err(<(String, String)>::from)?;
                            let Wire::Object(capabilities) = capabilities else {
                                return Err((
                                    "Fatal".to_owned(),
                                    "runtime/capabilities answer is not an object".to_owned(),
                                ));
                            };
                            let extra = (capabilities["workspaceExecutionCapabilities"] == true)
                                .then_some(("includeExecutionCapabilities", Value::Bool(true)));
                            runtime_call_with(
                                &instances,
                                &options,
                                "workspace/readPresentation",
                                params,
                                extra,
                                Extract::Result,
                            )
                            .await
                            .map_err(Into::into)
                        })
                    }
                }),
            ),
        ],
    )
}

/// `system`（TS `ServiceChannels.System` = `"system"`）：`info()` 返回
/// `{homedir, platform}`（TS `os.homedir()` + `process.platform`；Main 经 `rpcListen`
/// 给出的 home 与 Host 进程 HOME 同源）。终端 shell 探测与内网探测未实现。
pub fn system_channel(home: std::path::PathBuf) -> Channel {
    Channel::new(
        "system",
        vec![(
            "info",
            handler(move |_arg: Wire| {
                let home = home.clone();
                Box::pin(async move {
                    Ok(Wire::Object(json!({
                        "homedir": home.to_string_lossy(),
                        "platform": node_platform(),
                    })))
                })
            }),
        )],
    )
}

/// `process.platform` 口径（TS 用 Node 名字：darwin / win32 / linux）。
fn node_platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "darwin"
    } else if cfg!(target_os = "windows") {
        "win32"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else {
        std::env::consts::OS
    }
}

/// 组合根登记全部已迁移的 channel（Host 进程一处装配）。
/// `home` = 设置域根（homedir 语义）；`data_base_dir` = Main 提交的应用数据根（会话工作区用）。
pub fn channels(
    home: std::path::PathBuf,
    data_base_dir: Option<std::path::PathBuf>,
    instances: escode_host_runtime::Instances,
    options: escode_host_surface::OptionsFactory,
) -> Channels {
    let home_for_file = home.clone();
    let home_for_terminal = home.clone();
    let data_base_dir_for_file = data_base_dir.clone();
    // runtime 反向请求的 Host 侧应答（与 `setting` channel 同一份 settings 根）。
    instances.set_responder(crate::runtime_requests::responder(home.clone()));
    // 任务域与 session 域共用同一套实例表/装配工厂（同一 workspace 只会有一个实例），
    // 并**共用同一份**索引句柄 / 任务目标注册表 / 草稿注册表：Host 是索引投影的唯一写者，
    // 两条通道各持一份句柄会让「单写者 + 单份进程状态」失效（AGENTS.md 状态所有者）。
    let runtime = RuntimeAccess {
        instances: instances.clone(),
        options: options.clone(),
    };
    let index = Arc::new(Mutex::new(LazyIndex::default()));
    let targets = Arc::new(TaskTargets::new());
    let drafts = Arc::new(DeferredDrafts::new());
    // workspace 事件总线：任务域写面与 session 写面共用（Host 内唯一）。
    let events = Arc::new(crate::workspace_events::WorkspaceEvents::default());
    let task_channel = crate::domains_task::task_channel(
        data_base_dir.clone(),
        Some(runtime.clone()),
        Arc::clone(&index),
        Arc::clone(&targets),
        Arc::clone(&events),
    );
    let write = SessionWriteContext {
        runtime: runtime.clone(),
        read: data_base_dir.map(|data_base_dir| ReadContext {
            path: crate::domains_task_support::tasks_index_path(&data_base_dir),
            index,
            targets,
            runtime: Some(runtime),
        }),
        drafts,
        events: Arc::clone(&events),
    };
    // V4 面与任务索引 syncer 共用同一 Base（同一帧 hub / 实例表 / 索引 hint）。
    let base = crate::agent_v4::shared_base(instances.clone(), options.clone(), write.read.clone());
    let agent = crate::agent_v4::agent_channel_from_base(Arc::clone(&base));
    // syncer 只在能写索引时启动（缺 dataBaseDir 没有 V4 摄入的可落面）。
    if let Some(read) = write.read.clone() {
        crate::task_index_syncer::TaskIndexSyncer::spawn(
            Arc::clone(&base),
            read,
            Arc::clone(&events),
        );
    }
    Channels::new(vec![
        setting_channel(home.clone()),
        system_channel(home),
        crate::domains_file_channel::file_channel(home_for_file, data_base_dir_for_file),
        crate::domains_git::git_channel(),
        crate::domains_terminal::terminal_channel(home_for_terminal),
        task_channel,
        session_channel(instances, options, write),
    ])
    .with_scoped(agent)
}

#[cfg(test)]
#[path = "domains_tests.rs"]
mod tests;
