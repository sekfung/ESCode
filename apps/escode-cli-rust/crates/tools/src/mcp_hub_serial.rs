//! MCP hub 的串口部分（docs/specs/serial-agent-tools.md）：serial broker 生命周期、连接材料定向注入与会话上下文登记。
use super::Hub;
use crate::mcp_config::Server;
use std::sync::Arc;

impl Hub {
    fn serial_broker(&self) -> Option<Arc<crate::serial_broker::SerialBroker>> {
        self.serial
            .get_or_init(|| crate::serial_broker::SerialBroker::start(self.host.clone()))
            .clone()
    }
    /// 只向 serial server 注入 broker 的 socket 与 token；其它 MCP / Bash 子进程拿不到串口通道。
    pub(super) fn inject_serial_broker(&self, server: &mut Server) {
        if server.name != crate::mcp_serial::NAME {
            return;
        }
        if let Some(broker) = self.serial_broker()
            && let (Some(env), Some(extra)) = (server.raw["env"].as_object_mut(), broker.env().as_object())
        {
            env.extend(extra.clone());
        }
    }
    /// 登记本次 MCP 调用的会话上下文，broker 据此确认会话归属并补齐 workspace 身份（TS requireSession）。
    pub(super) fn remember_serial(&self, session: &str, meta: &serde_json::Value) {
        if let Some(Some(broker)) = self.serial.get() {
            broker.remember(session, meta);
        }
    }
}
