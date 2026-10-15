//! MCP hub 的宿主工具部分：serial（docs/specs/serial-agent-tools.md）的 broker 生命周期、
//! 连接材料定向注入与会话上下文登记。
use super::Hub;
use crate::host_broker::{HostBroker, SERIAL};
use crate::mcp_config::Server;
use std::sync::{Arc, OnceLock};

type BrokerSlot = OnceLock<Option<Arc<HostBroker>>>;

impl Hub {
    /// 按 server 名找到对应 broker 槽位；首次出现该 server 配置时才启动 broker。
    fn host_broker(&self, server: &str) -> Option<Arc<HostBroker>> {
        let (slot, spec): (&BrokerSlot, _) = match server {
            crate::mcp_serial::NAME => (&self.serial, &SERIAL),
            _ => return None,
        };
        slot.get_or_init(|| HostBroker::start(spec, self.host.clone()))
            .clone()
    }
    /// 只向 serial server 注入它的 broker socket 与 token；其它 MCP / Bash 子进程拿不到 Host 通道。
    pub(super) fn inject_host_broker(&self, server: &mut Server) {
        if let Some(broker) = self.host_broker(&server.name)
            && let (Some(env), Some(extra)) =
                (server.raw["env"].as_object_mut(), broker.env().as_object())
        {
            env.extend(extra.clone());
        }
    }
    /// 登记本次 MCP 调用的会话上下文，broker 据此确认会话归属并补齐 workspace 身份（TS requireSession）。
    pub(super) fn remember_host_brokers(&self, session: &str, meta: &serde_json::Value) {
        for slot in [&self.serial] {
            if let Some(Some(broker)) = slot.get() {
                broker.remember(session, meta);
            }
        }
    }
}
