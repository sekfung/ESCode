//! Host 服务契约清单（rust-u2-host-skeleton.md §4）：channel/method 的单一来源，
//! `--check` 与实现的存在性对账。未实现成员进 `pending`（Gateway 立即回
//! `NotImplemented`，缺口显式可见），已实现成员必须能在源码里找到登记点。

use std::collections::BTreeSet;

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

/// 契约清单（u2a 起点：先登记阶段 2 优先序 3–5 的服务面；迁移一域补一域）。
pub const METHODS: &[Method] = &[
    // 通路（u2a 自带）
    Method {
        channel: "control",
        method: "host/attachWindow",
        state: State::Pending,
    },
    Method {
        channel: "control",
        method: "host/detachWindow",
        state: State::Pending,
    },
    Method {
        channel: "control",
        method: "host/shutdown",
        state: State::Pending,
    },
    // settings（优先序 3）
    Method {
        channel: "settings",
        method: "settings/read",
        state: State::Pending,
    },
    Method {
        channel: "settings",
        method: "settings/write",
        state: State::Pending,
    },
    // workspace（优先序 4）
    Method {
        channel: "workspace",
        method: "workspace/query",
        state: State::Pending,
    },
    // sessions + tasks（优先序 5）
    Method {
        channel: "v4/conversation",
        method: "v4/conversation/subscribe",
        state: State::Pending,
    },
    Method {
        channel: "v4/conversation",
        method: "v4/conversation/resync",
        state: State::Pending,
    },
];

/// 清单里的全部 channel（排序去重）。
pub fn channels() -> Vec<&'static str> {
    METHODS
        .iter()
        .map(|entry| entry.channel)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// 某 channel 的 method 清单（保持声明序）。
pub fn methods_of(channel: &str) -> Vec<&'static Method> {
    METHODS
        .iter()
        .filter(|entry| entry.channel == channel)
        .collect()
}

/// 清单自检：无重复 (channel, method) 对。
pub fn duplicates() -> Vec<(&'static str, &'static str)> {
    let mut seen = BTreeSet::new();
    let mut duplicates = Vec::new();
    for entry in METHODS {
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
    let missing: Vec<_> = METHODS
        .iter()
        .filter(|entry| entry.state == State::Implemented)
        .filter(|entry| !implemented.contains(&(entry.channel, entry.method)))
        .map(|entry| (entry.channel, entry.method))
        .collect();
    anyhow::ensure!(
        missing.is_empty(),
        "Host contract marks methods implemented but no implementation registers them: {missing:?}"
    );
    let claimed: BTreeSet<_> = METHODS
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
        assert_eq!(methods_of("control").len(), 3);
    }

    #[test]
    fn check_accepts_a_matching_registration_set() {
        let implemented: BTreeSet<_> = METHODS
            .iter()
            .filter(|entry| entry.state == State::Implemented)
            .map(|entry| (entry.channel, entry.method))
            .collect();
        check(&implemented).expect("登记集合与清单一致时通过");
    }

    #[test]
    fn check_rejects_unregistered_implementations_and_unknown_methods() {
        // 负向验证：清单里一旦有 Implemented 成员而登记集为空，必须失败。
        if METHODS
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
        let mut extra = BTreeSet::new();
        extra.insert(("settings", "settings/anything"));
        let error = check(&extra).expect_err("未知方法必须失败");
        assert!(
            error
                .to_string()
                .contains("missing from the contract inventory")
        );
    }
}
