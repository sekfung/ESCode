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

/// RPC 面的通道集合 = TS 服务面全集里的 channel（`host_proto::TS_SERVICE_CHANNELS`，生成资产）。
/// `control` 归 Host 进程入口（另有 host-proto 内的对账测试）。
fn rpc_channels() -> BTreeSet<&'static str> {
    escode_host_proto::TS_SERVICE_CHANNELS
        .iter()
        .map(|(channel, _)| *channel)
        .collect()
}

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
        .filter(|entry| rpc_channels().contains(entry.channel))
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
fn registered_channels_are_exactly_those_with_implemented_members() {
    // 有已实现成员的 channel 必须注册；一个成员都没有的 channel 不注册（由 `respond` 回显式缺口）。
    let expected: BTreeSet<&str> = manifest_with(State::Implemented)
        .into_iter()
        .map(|(channel, _)| {
            rpc_channels()
                .into_iter()
                .find(|c| *c == channel)
                .expect("rpc channel")
        })
        .collect();
    let channels = live_channels();
    let registered: BTreeSet<&str> = channels
        .names()
        .into_iter()
        .filter(|name| rpc_channels().contains(name))
        .collect();
    assert_eq!(
        registered, expected,
        "注册的 RPC channel 与清单里有已实现成员的 channel 不一致"
    );
    // 注册名都必须是 TS 真实 channel（防拼错）。
    let unknown: Vec<_> = channels
        .names()
        .into_iter()
        .filter(|name| !rpc_channels().contains(name))
        .collect();
    assert!(
        unknown.is_empty(),
        "注册了 TS 服务面不存在的 channel: {unknown:?}"
    );
    assert!(
        rpc_channels().len() >= 50,
        "TS 服务面全集应覆盖全部 ServiceChannels"
    );
}
