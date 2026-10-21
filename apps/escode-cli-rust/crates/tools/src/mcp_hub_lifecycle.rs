//! Hub 的生命周期收口：连接回收与会话/进程清理（`impl Hub` 的拆分子模块）。
//!
//! 连接回收时同步清理 MCP App 的连接来源，避免旧凭证据此继续通过校验。
use super::*;

impl Hub {
    pub(super) async fn prune(&self) -> Result<()> {
        let removed = {
            let mut state = self.state_mut();
            let used = state
                .bindings
                .values()
                .flatten()
                .map(|b| b.key.clone())
                .collect::<BTreeSet<_>>();
            // 借用检查：keys() 迭代器借住 connections，remove 需要独占借用，必须先收集 key 再删除
            // （clippy::needless_collect 的"直接用迭代器"建议在此无法通过编译）。
            #[allow(clippy::needless_collect)]
            let keys = state
                .connections
                .keys()
                .filter(|k| !used.contains(*k))
                .cloned()
                .collect::<Vec<_>>();
            // 连接被回收后同一 server 的旧来源不再可信（MCP App 凭证据此换代）。
            for sources in state.sources.values_mut() {
                sources.retain(|_, source| !keys.contains(&source.identity));
            }
            keys.into_iter()
                .filter_map(|k| state.connections.remove(&k))
                .collect::<Vec<_>>()
        };
        for connection in removed {
            connection.close().await?;
        }
        Ok(())
    }
    pub async fn shutdown(&self) -> Result<()> {
        self.stop.cancel();
        let _gate = self.gate.lock().await;
        {
            let mut state = self.state_mut();
            state.bindings.clear();
            state.overrides.clear();
            state.sources.clear();
        }
        *self.ui() = Default::default();
        self.prune().await
    }
}
