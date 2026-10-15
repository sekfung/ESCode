//! 契约清单本体（channel/method 与实现状态）。新增控制消息或服务方法必须在这里登记。
//! 服务域条目在 `inventory_services.rs`（单文件行数上限），`methods()` 是全量视图。

use crate::Method;
use crate::State;

use crate::inventory_services::service_methods;

/// 通路方法清单（control 通道：进程入口、窗口 attach/request、日志与容量控制）。
/// 服务域条目在 `inventory_services.rs`，两者由 `methods()` 合成。
/// `control` 通道的方法名即 `ControlMessage` 的 serde tag（漂移由 host-proto 的
/// `control_inventory_matches_the_protocol_enum` 测试拦截）。
pub const CONTROL_METHODS: &[Method] = &[
    // 通路（u2a 自带；Process = 进程入口的控制循环已实现）
    Method {
        channel: "control",
        method: "hostReady",
        state: State::Implemented,
    },
    Method {
        channel: "control",
        method: "windowAttach",
        state: State::Implemented,
    },
    Method {
        channel: "control",
        method: "windowAttached",
        state: State::Implemented,
    },
    Method {
        channel: "control",
        method: "windowRequest",
        state: State::Implemented,
    },
    Method {
        channel: "control",
        method: "windowFrame",
        state: State::Implemented,
    },
    Method {
        channel: "control",
        method: "windowDetach",
        state: State::Implemented,
    },
    Method {
        channel: "control",
        method: "windowDetached",
        state: State::Implemented,
    },
    Method {
        channel: "control",
        method: "logBatch",
        state: State::Implemented,
    },
    Method {
        channel: "control",
        method: "failed",
        state: State::Implemented,
    },
    Method {
        channel: "control",
        method: "shutdown",
        state: State::Implemented,
    },
    Method {
        channel: "control",
        method: "stopped",
        state: State::Implemented,
    },
];

/// 契约清单全量视图（control + 服务域）：契约对账与能力清单的唯一来源。
pub fn methods() -> Vec<Method> {
    let mut all = CONTROL_METHODS.to_vec();
    all.extend(service_methods());
    all
}
