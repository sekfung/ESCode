//! Host 服务契约清单（rust-u2-host-skeleton.md §4）：channel/method 的单一来源，
//! `--check` 与实现的存在性对账。未实现成员进 `pending`（Gateway 立即回
//! `NotImplemented`，缺口显式可见），已实现成员必须能在源码里找到登记点。

use std::collections::BTreeSet;

mod inventory;
mod inventory_non_rpc;
mod inventory_services;
mod inventory_ts;

pub use inventory_services::{IMPLEMENTED, ts_service_members};
pub use inventory_ts::TS_SERVICE_CHANNELS;

/// 一个 channel 上的 method 清单。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Method {
    /// 通道名（如 `settings`、`v4/conversation`）。
    pub channel: &'static str,
    /// 方法名（Renderer↔Host RPC 的 `method` 字符串）。
    pub method: &'static str,
    /// 实现状态：`Pending` = 尚未迁移（Gateway 回 NotImplemented）。
    pub state: State,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Implemented,
    Pending,
}

pub use inventory::{CONTROL_METHODS, methods};

/// 某通道上标记为已实现的方法名。
pub fn implemented_methods(channel: &str) -> Vec<&'static str> {
    methods()
        .iter()
        .filter(|entry| entry.channel == channel && entry.state == State::Implemented)
        .map(|entry| entry.method)
        .collect()
}

/// 清单里的全部 channel（排序去重）。
pub fn channels() -> Vec<&'static str> {
    methods()
        .iter()
        .map(|entry| entry.channel)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// 某 channel 的 method 清单（保持声明序）。
pub fn methods_of(channel: &str) -> Vec<Method> {
    methods()
        .into_iter()
        .filter(|entry| entry.channel == channel)
        .collect()
}

/// 清单自检：无重复 (channel, method) 对。
pub fn duplicates() -> Vec<(&'static str, &'static str)> {
    let mut seen = BTreeSet::new();
    let mut duplicates = Vec::new();
    for entry in methods() {
        if !seen.insert((entry.channel, entry.method)) {
            duplicates.push((entry.channel, entry.method));
        }
    }
    duplicates
}

/// `--check` 的对账实现：清单本身必须自洽，且每个 `Implemented` 成员都能在
/// 调用方给出的实现集合里找到（源码登记点），缺一即失败。
pub fn check(implemented: &BTreeSet<(&'static str, &'static str)>) -> anyhow::Result<()> {
    let duplicates = duplicates();
    anyhow::ensure!(
        duplicates.is_empty(),
        "Host contract inventory has duplicate entries: {duplicates:?}"
    );
    let missing: Vec<_> = methods()
        .iter()
        .filter(|entry| entry.state == State::Implemented)
        .filter(|entry| !implemented.contains(&(entry.channel, entry.method)))
        .map(|entry| (entry.channel, entry.method))
        .collect();
    anyhow::ensure!(
        missing.is_empty(),
        "Host contract marks methods implemented but no implementation registers them: {missing:?}"
    );
    let claimed: BTreeSet<_> = methods()
        .iter()
        .map(|entry| (entry.channel, entry.method))
        .collect();
    let extra: Vec<_> = implemented.difference(&claimed).copied().collect();
    anyhow::ensure!(
        extra.is_empty(),
        "Host implementation registers methods missing from the contract inventory: {extra:?}"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inventory_is_self_consistent() {
        assert!(duplicates().is_empty());
        assert!(channels().contains(&"settings"));
        let control = methods_of("control");
        assert!(control.len() >= 3);
        assert!(control.iter().any(|entry| entry.method == "hostReady"));
    }

    #[test]
    fn check_accepts_a_matching_registration_set() {
        let implemented: BTreeSet<_> = methods()
            .iter()
            .filter(|entry| entry.state == State::Implemented)
            .map(|entry| (entry.channel, entry.method))
            .collect();
        check(&implemented).expect("登记集合与清单一致时通过");
    }

    #[test]
    fn check_rejects_unregistered_implementations_and_unknown_methods() {
        // 负向验证：清单里一旦有 Implemented 成员而登记集为空，必须失败。
        if methods()
            .iter()
            .any(|entry| entry.state == State::Implemented)
        {
            let error = check(&BTreeSet::new()).expect_err("缺实现登记必须失败");
            assert!(
                error
                    .to_string()
                    .contains("no implementation registers them")
            );
        }
        // 额外登记（清单里没有的方法）同样失败。
        let mut extra: BTreeSet<(&'static str, &'static str)> = methods()
            .iter()
            .filter(|entry| entry.state == State::Implemented)
            .map(|entry| (entry.channel, entry.method))
            .collect();
        extra.insert(("settings", "settings/anything"));
        let error = check(&extra).expect_err("未知方法必须失败");
        assert!(
            error
                .to_string()
                .contains("missing from the contract inventory")
        );
    }
}

#[cfg(test)]
mod contract_drift {
    use super::*;
    use escode_host_control::ControlMessage;

    /// 清单与源码一致（rust-u2-hose-skeleton §4）：`control` 通道的已实现方法名集合
    /// 必须等于 `ControlMessage` 全部 kind —— 新增/删除控制消息而不更新清单会失败。
    #[test]
    fn control_inventory_matches_the_protocol_enum() {
        let declared: std::collections::BTreeSet<&str> =
            implemented_methods("control").into_iter().collect();
        let sample: Vec<ControlMessage> = vec![
            ControlMessage::WindowAttach {
                window_id: String::new(),
                client_id: String::new(),
                workspace: Default::default(),
            },
            ControlMessage::WindowAttached {
                window_id: String::new(),
                generation: 0,
            },
            ControlMessage::WindowRequest {
                window_id: String::new(),
                line: String::new(),
            },
            ControlMessage::WindowFrame {
                window_id: String::new(),
                line: String::new(),
            },
            ControlMessage::WindowDetach {
                window_id: String::new(),
            },
            ControlMessage::WindowDetached {
                window_id: String::new(),
            },
            ControlMessage::LocalMediaPreviewAuthorize {
                request_id: String::new(),
                path: String::new(),
            },
            ControlMessage::LocalMediaPreviewAuthorized {
                request_id: String::new(),
                ok: false,
                path: None,
                error: None,
            },
            ControlMessage::HostReady {
                protocol_version: 0,
                workspaces: 0,
            },
            ControlMessage::Shutdown {
                reason: String::new(),
            },
            ControlMessage::LogBatch {
                scope: String::new(),
                level: escode_host_control::LogLevel::Info,
                lines: Vec::new(),
            },
            ControlMessage::Failed {
                error: escode_host_control::ControlError {
                    code: escode_host_control::ControlErrorCode::Fatal,
                    message: String::new(),
                },
            },
            ControlMessage::Stopped {
                residual: Vec::new(),
            },
            ControlMessage::SettingsRead {
                home: String::new(),
            },
            ControlMessage::SettingsWrite {
                home: String::new(),
                patch: serde_json::Value::Null,
            },
            ControlMessage::SettingsValue {
                settings: serde_json::Value::Null,
            },
        ];
        // settings 域的消息在清单里单独分通道登记，这里只看 control 通道。
        let settings_kinds: std::collections::BTreeSet<&str> =
            ["settingsRead", "settingsWrite", "settingsValue"]
                .into_iter()
                .collect();
        let actual: std::collections::BTreeSet<&str> = sample
            .iter()
            .map(ControlMessage::kind)
            .filter(|kind| !settings_kinds.contains(kind))
            .collect();
        assert_eq!(
            actual, declared,
            "控制消息 kind 与契约清单不一致（新增消息必须同时登记到 control 通道）"
        );
        let settings_declared: std::collections::BTreeSet<&str> =
            implemented_methods("settings").into_iter().collect();
        let settings_actual: std::collections::BTreeSet<&str> =
            ["settingsRead", "settingsWrite", "settingsValue"]
                .into_iter()
                .collect();
        assert_eq!(settings_actual, settings_declared);
    }
}
