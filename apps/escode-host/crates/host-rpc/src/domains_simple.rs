//! 不依赖会话/索引状态的“薄服务”channel 装配（家目录/数据目录文件视图、凭据、插件管理转发）。
//! 从 `domains.rs` 拆出以守住单文件行数上限；新增同类域在这里登记。

use std::path::Path;

use escode_host_runtime::Instances;

use crate::channel::Channel;

pub(crate) fn simple_channels(
    home: &Path,
    data_base_dir: Option<&Path>,
    instances: Instances,
    options: escode_host_surface::OptionsFactory,
) -> Vec<Channel> {
    let mut channels = vec![
        crate::domains_broadcast::broadcast_channel(std::sync::Arc::new(
            escode_host_services::broadcast::Broadcast::default(),
        )),
        crate::domains_memory::memory_channel(home, data_base_dir),
        crate::domains_output_style::output_style_channel(home),
        crate::domains_attachments::prompt_attachment_transfer_channel(),
        crate::domains_attachments::media_preview_channel(),
        crate::domains_credential::credential_channel(data_base_dir.map(Path::to_path_buf)),
        crate::domains_plugin::plugin_management_channel(
            instances,
            options,
            data_base_dir.map(Path::to_path_buf),
        ),
    ];
    channels.push(crate::domains_onboarding::onboarding_channel(data_base_dir));
    channels
}
