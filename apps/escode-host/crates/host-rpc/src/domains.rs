//! 已迁移的服务域：channels 表在这里登记（rust-u2-host-skeleton.md §5 优先序）。
//! 未登记的 channel 由 `session::respond` 回显式 `NotImplemented`。

use std::sync::{Arc, Mutex};

use serde_json::json;

use crate::deferred_drafts::DeferredDrafts;
use crate::domains_session::session_channel;
use crate::domains_session_write::SessionWriteContext;
use crate::domains_task_support::{LazyIndex, ReadContext, RuntimeAccess};
use crate::session::{Channel, Channels, handler};
use crate::task_targets::TaskTargets;
use crate::wire::Wire;

/// `settingService`（TS `ServiceChannels.Setting` = `"setting"`）：
/// `get` 读设置文件、`update` 顶层合并补丁后原子写（与 Node Host 同文件同格式）；
/// 其余方法（`updateDataBaseDir` / `ensureDefaultProject`）未实现——方法表即分派，
/// 缺口由 `respond` 回显式 `NotImplemented` 并列出已实现方法。
pub fn setting_channel(
    home: std::path::PathBuf,
    data_base_dir: Option<std::path::PathBuf>,
) -> Channel {
    let extra = crate::domains_setting_extra::extra_methods(&home, data_base_dir.as_deref());
    let read_home = home.clone();
    let write_home = home;
    let mut methods = vec![
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
    ];
    methods.extend(extra);
    Channel::new("setting", methods)
}

/// `system`（TS `ServiceChannels.System` = `"system"`）：`info()` 返回
/// `{homedir, platform}`（TS `os.homedir()` + `process.platform`；Main 经 `rpcListen`
/// 给出的 home 与 Host 进程 HOME 同源）。终端 shell 探测与内网探测未实现。
pub fn system_channel(home: std::path::PathBuf) -> Channel {
    let mut methods = vec![(
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
    )];
    methods.extend(crate::domains_system::extra_methods());
    Channel::new("system", methods)
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
    channels_with(home, data_base_dir, instances, options, None, None, None)
}

/// 同 [`channels`]，并带 provider 配置文件路径（Built-in, Personal；由 Main 经 `rpcListen.runtimeEnv` 提供）。
pub fn channels_with(
    home: std::path::PathBuf,
    data_base_dir: Option<std::path::PathBuf>,
    instances: escode_host_runtime::Instances,
    options: escode_host_surface::OptionsFactory,
    provider_files: Option<escode_host_services::provider::ProviderFiles>,
    media_authorizer: Option<Arc<escode_host_control::path_authorizer::PathAuthorizer>>,
    client_services: Option<Arc<escode_host_services::client_services::ClientServices>>,
) -> Channels {
    let data_base_dir_for_setting = data_base_dir.clone();
    let provider_service = provider_files.map(|files| {
        let service = Arc::new(files.into_service());
        // Main 声明了 Remote 刷新才会有 synchronizer；否则这里是空操作（rust-u2-providers.md §12）。
        service.start_remote_refresh();
        service
    });
    let home_for_simple = home.clone();
    let data_base_dir_for_simple = data_base_dir.clone();
    let instances_for_simple = instances.clone();
    let options_for_simple = Arc::clone(&options);
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
    let task_channel = task_channel.with_methods(
        crate::domains_task_trajectory::trajectory_methods(&home, data_base_dir.as_deref()),
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
    // 窗口控制器（window-controller）与它共用同一实例：会话摘要基线是 controller overlay 的唯一源。
    let syncer = write.read.clone().map(|read| {
        crate::task_index_syncer::TaskIndexSyncer::spawn(
            Arc::clone(&base),
            read,
            Arc::clone(&events),
        )
    });
    let controller = write.read.clone().zip(syncer).map(|(read, syncer)| {
        crate::window_controller::WindowController::new(read, Arc::clone(&events), syncer)
    });
    let mut list = vec![
        setting_channel(home.clone(), data_base_dir_for_setting),
        system_channel(home),
        crate::domains_file_channel::file_channel(home_for_file, data_base_dir_for_file),
        crate::domains_file_watcher::file_watcher_channel(),
        crate::domains_git::git_channel(
            provider_service.clone(),
            instances_for_simple.clone(),
            Arc::clone(&options_for_simple),
        ),
        crate::domains_terminal::terminal_channel(home_for_terminal),
        task_channel,
        session_channel(instances, options, write),
    ];
    list.extend(crate::domains_provider::provider_channels(
        provider_service,
        instances_for_simple.clone(),
        Arc::clone(&options_for_simple),
    ));
    list.extend(crate::domains_simple::simple_channels(
        &home_for_simple,
        data_base_dir_for_simple.as_deref(),
        instances_for_simple,
        options_for_simple,
        media_authorizer,
        client_services,
    ));
    let channels = Channels::new(list);
    // 控制器是连接级 channel（每连接一份订阅表）；缺 dataBaseDir 时同址回显式缺口。
    let channels = match controller {
        Some(controller) => {
            channels.with_scoped(crate::window_controller::controller_channel(controller))
        }
        None => channels.with_scoped(std::sync::Arc::new(|_context| {
            crate::window_controller::gap_channel()
        })),
    };
    channels.with_scoped(agent)
}

#[cfg(test)]
#[path = "domains_tests.rs"]
mod tests;
