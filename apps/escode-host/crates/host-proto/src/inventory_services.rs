//! 契约清单的**服务域**条目：Renderer RPC 面以 TS 服务面全集为准（`inventory_ts.rs`，由
//! `scripts/generate-escode-host-service-inventory.mjs` 从源码生成），这里只登记**已实现**的成员——
//! 其余 TS 成员一律 Pending（评审 A13：旧清单手工登记 9 个 channel，未登记 channel 的缺口不可见，
//! 且有拼错的条目 `file.workspaceFileListLength`，TS 实名 `listWorkspaceFilesLength`）。

use crate::inventory_implemented::IMPLEMENTED;
use crate::inventory_non_rpc::NON_RPC_METHODS;
use crate::inventory_ts::TS_SERVICE_CHANNELS;
use crate::{Method, State};

/// TS 服务面全集的扁平视图 `(channel, member)`。
pub fn ts_service_members() -> impl Iterator<Item = (&'static str, &'static str)> {
    TS_SERVICE_CHANNELS
        .iter()
        .flat_map(|&(channel, members)| members.iter().map(move |&member| (channel, member)))
}

/// 服务域全量视图：非 RPC 条目 + TS 服务面（已实现/Pending）。
pub fn service_methods() -> Vec<Method> {
    let mut all = NON_RPC_METHODS.to_vec();
    all.extend(ts_service_members().map(|(channel, method)| Method {
        channel,
        method,
        state: if IMPLEMENTED.contains(&(channel, method)) {
            State::Implemented
        } else {
            State::Pending
        },
    }));
    all
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn implemented_entries_are_real_typescript_members() {
        let fabricated: Vec<_> = IMPLEMENTED
            .iter()
            .filter(|&&entry| !ts_service_members().any(|member| member == entry))
            .collect();
        assert!(
            fabricated.is_empty(),
            "IMPLEMENTED 里有 TS 服务面不存在的成员（拼错或已删除）: {fabricated:?}"
        );
    }
}
