//! 契约清单 ↔ 源码双向对账（rust-unified-host.md 阶段 2 验收：
//! 「Host 服务契约清单（channel/method）`--check` 与源码一致」）。
//!
//! `host-rpc` 的 channel 方法表就是分派本身（表里没有的方法一律回 `NotImplemented`），
//! 因此这份对账等价于"清单与源码一致"：
//!
//! 1. 方法表里的每个 (channel, method) 必须在清单里是 `Implemented`；
//! 2. 清单里标 `Implemented` 的 RPC 面方法必须能在方法表里找到（否则渲染器永远拿不到它）；
//! 3. 清单里标 `Pending` 的 RPC 面方法**不得**出现在方法表里（缺口必须仍然显式）。
use std::collections::BTreeSet;

use escode_host_proto::State;

/// RPC 面（`host-rpc` 提供，渲染器经 `/ws` 调用）的通道集合。
/// `control` 归 Host 进程入口（另有 host-proto 内的对账测试）。
const RPC_CHANNELS: [&str; 5] = ["setting", "system", "file", "escode-task", "escode-session"];

fn live_channels() -> escode_host_rpc::Channels {
    let instances = escode_host_runtime::Instances::new(escode_host_runtime::Link::Embedded);
    let options: escode_host_surface::OptionsFactory =
        std::sync::Arc::new(|workspace: &escode_host_control::WorkspaceDescriptor| {
            escode_cli_rust::embed::RuntimeOptions {
                home: std::path::PathBuf::from("/tmp"),
                env: std::collections::BTreeMap::new(),
                argv: Vec::new(),
                cwd: std::path::PathBuf::from(&workspace.path),
                workspace_identity: workspace.identity.clone(),
                data_dir: None,
                config: None,
            }
        });
    escode_host_rpc::domains::channels(std::path::PathBuf::from("/tmp"), None, instances, options)
}

/// 清单里某个状态的 RPC 面 (channel, method) 集合。
fn manifest_with(state: State) -> BTreeSet<(String, String)> {
    escode_host_proto::methods()
        .iter()
        .filter(|entry| RPC_CHANNELS.contains(&entry.channel))
        .filter(|entry| entry.state == state)
        .map(|entry| (entry.channel.to_owned(), entry.method.to_owned()))
        .collect()
}

fn implemented_in_source() -> BTreeSet<(String, String)> {
    live_channels()
        .inventory()
        .into_iter()
        .map(|(channel, method)| (channel.to_owned(), method.to_owned()))
        .collect()
}

#[test]
fn source_method_table_matches_implemented_entries() {
    let source = implemented_in_source();
    let declared = manifest_with(State::Implemented);
    let missing: Vec<_> = source.difference(&declared).cloned().collect();
    assert!(
        missing.is_empty(),
        "方法表里实现了但清单未标 Implemented: {missing:?}"
    );
    let undeclared: Vec<_> = declared.difference(&source).cloned().collect();
    assert!(
        undeclared.is_empty(),
        "清单标了 Implemented 但方法表里没有（渲染器永远拿不到）: {undeclared:?}"
    );
}

#[test]
fn pending_entries_are_not_implemented() {
    let source = implemented_in_source();
    let pending = manifest_with(State::Pending);
    let leaked: Vec<_> = pending.intersection(&source).cloned().collect();
    assert!(
        leaked.is_empty(),
        "清单标 Pending 但方法表已实现（缺口清单过时）: {leaked:?}"
    );
    assert!(!pending.is_empty(), "缺口清单必须非空（迁移期）");
}

#[test]
fn every_rpc_channel_is_registered() {
    let channels = live_channels();
    for channel in RPC_CHANNELS {
        assert!(
            channels.channel(channel).is_some(),
            "RPC 通道 {channel} 未在 host-rpc 注册"
        );
    }
    // 清单只登记这些 RPC 通道（新增通道必须同步本测试的 RPC_CHANNELS 与清单）。
    let manifest_channels: BTreeSet<&str> = escode_host_proto::methods()
        .iter()
        .map(|entry| entry.channel)
        .filter(|channel| RPC_CHANNELS.contains(channel))
        .collect();
    assert_eq!(
        manifest_channels.len(),
        RPC_CHANNELS.len(),
        "清单 RPC 通道数与预期不一致: {manifest_channels:?}"
    );
}
