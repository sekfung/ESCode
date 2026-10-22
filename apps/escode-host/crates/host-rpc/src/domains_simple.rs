//! 不依赖会话/索引状态的“薄服务”channel 装配（家目录/数据目录文件视图、凭据、插件管理转发）。
//! 从 `domains.rs` 拆出以守住单文件行数上限；新增同类域在这里登记。

use std::path::Path;

use escode_host_runtime::Instances;

use crate::channel::Channel;

pub(crate) fn simple_channels(
    home: &Path,
    data_base_dir: Option<&Path>,
    instances: Instances,
    plugin_management: crate::domains_plugin::PluginManagement,
    media_authorizer: Option<std::sync::Arc<escode_host_control::path_authorizer::PathAuthorizer>>,
    main_broker: Option<std::sync::Arc<escode_host_control::main_broker::MainBroker>>,
    client_services: Option<std::sync::Arc<escode_host_services::client_services::ClientServices>>,
) -> Vec<Channel> {
    let usage_stats_instances = instances;
    let mut channels = vec![
        crate::domains_broadcast::broadcast_channel(std::sync::Arc::new(
            escode_host_services::broadcast::Broadcast::default(),
        )),
        crate::domains_memory::memory_channel(home, data_base_dir),
        crate::domains_commands::commands_channel(home.to_path_buf()),
        crate::domains_subagents::subagents_channel(home.to_path_buf()),
        crate::domains_skills::skills_channel(home),
        crate::domains_output_style::output_style_channel(home),
        crate::domains_attachments::prompt_attachment_transfer_channel(),
        crate::domains_attachments::media_preview_channel(media_authorizer),
        crate::domains_client::client_scenes_channel(client_services.clone()),
        crate::domains_client::client_config_channel(client_services),
        crate::domains_credential::credential_channel(data_base_dir.map(Path::to_path_buf)),
        plugin_management.channel(),
        // 旧 `plugins` 通道：整套退役，纯常量应答（无依赖）。
        crate::domains_plugins_legacy::plugins_channel(),
    ];
    channels.push(crate::domains_onboarding::onboarding_channel(data_base_dir));
    // `gen-ui`：模型写可视化文件、Host 受限读取；widget 状态与通知同源。
    let cua_broker = main_broker.clone();
    channels.push(crate::domains_gen_ui::gen_ui_channel(
        data_base_dir.map(std::path::Path::to_path_buf),
        main_broker,
    ));
    // `cua-permission`：Helper 生命周期归 Main，Host 只经 mainRequest 转发。
    channels.push(crate::domains_cua_permission::cua_permission_channel(
        cua_broker,
    ));
    // `usage-stats` 只读实例表（不按 workspace 路由、不启动运行时），与其余域无共享状态。
    channels.push(crate::domains_usage_stats::usage_stats_channel(
        usage_stats_instances,
    ));
    channels
}
