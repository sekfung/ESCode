//! 不属于 Renderer RPC 面的服务条目（Main↔Host 控制面与 u2a 占位）。

use crate::{Method, State};

/// 不属于 Renderer RPC 面的服务条目（不在 TS 服务描述符里）：Main↔Host 控制面的 settings 消息、
/// u2a 通路的 workspace / v4 占位。
pub const NON_RPC_METHODS: &[Method] = &[
    Method {
        channel: "settings",
        method: "settingsRead",
        state: State::Implemented,
    },
    Method {
        channel: "settings",
        method: "settingsWrite",
        state: State::Implemented,
    },
    Method {
        channel: "settings",
        method: "settingsValue",
        state: State::Implemented,
    },
    Method {
        channel: "workspace",
        method: "workspace/query",
        state: State::Pending,
    },
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
