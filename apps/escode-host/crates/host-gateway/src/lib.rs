//! Gateway：连接注册表与到达序 lane（rust-u2-host-skeleton.md §3.4）。
//!
//! 两条不可让步的规则：
//! 1. **登记必须在请求到达时同步完成**：`register` 在调用线程直接取号建 lane，
//!    不经过任务调度；lane 顺序 = accept/read 到达序。
//! 2. **每连接一个写者**：出站帧按 lane 顺序排列，`send` 只 append 到本连接 lane。
//!    capability 未覆盖的 method 立即回 `NotImplemented`，Renderer 不挂起。

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use escode_host_core::bounded::{self, Overflow, Sender};
use tokio::sync::mpsc;

/// 连接号：进程内单调，决定 lane 顺序（注册顺序）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ConnectionId(pub u64);

/// 一条连接的出站 lane：有界队列 + 水位计数。
pub struct Lane {
    pub id: ConnectionId,
    pub outbound: Sender<serde_json::Value>,
    /// 对端声明的能力集合（握手结果）。
    pub capabilities: Capabilities,
}

/// 客户端能力 ∩ Host 支持面。空集 = 只能收控制帧与错误。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Capabilities {
    methods: std::collections::BTreeSet<String>,
}

impl Capabilities {
    /// 按客户端声明与 Host 支持面求交（不支持的声明不登记）。
    pub fn negotiate(requested: &[String], supported: &[&str]) -> Self {
        let supported: std::collections::BTreeSet<&str> = supported.iter().copied().collect();
        Self {
            methods: requested
                .iter()
                .filter(|method| supported.contains(method.as_str()))
                .cloned()
                .collect(),
        }
    }

    pub fn supports(&self, method: &str) -> bool {
        self.methods.contains(method)
    }

    pub fn methods(&self) -> Vec<&str> {
        self.methods.iter().map(String::as_str).collect()
    }
}

#[derive(Default)]
struct State {
    next: u64,
    lanes: BTreeMap<ConnectionId, Lane>,
}

/// 连接注册表。`register` 同步取号 —— 到达序即 lane 序。
#[derive(Clone, Default)]
pub struct Gateway {
    state: Arc<Mutex<State>>,
}

impl Gateway {
    pub fn new() -> Self {
        Self::default()
    }

    /// 同步登记一条连接（调用方在 accept/read 的到达点直接调用）。
    /// 返回 lane 与它的出站接收端：**每连接一个写者**，接收端只能被该写者持有。
    pub fn register(
        &self,
        capabilities: Capabilities,
        outbound_capacity: usize,
    ) -> (Lane, mpsc::Receiver<serde_json::Value>) {
        let mut state = self.lock();
        let id = ConnectionId(state.next);
        state.next += 1;
        let (outbound, receiver) = bounded::channel::<serde_json::Value>(
            "gateway-outbound",
            outbound_capacity,
            Overflow::DropReconstructible,
        );
        let lane = Lane {
            id,
            outbound,
            capabilities,
        };
        state.lanes.insert(id, lane.clone());
        (lane, receiver)
    }

    pub fn unregister(&self, id: ConnectionId) -> bool {
        self.lock().lanes.remove(&id).is_some()
    }

    pub fn connections(&self) -> Vec<ConnectionId> {
        self.lock().lanes.keys().copied().collect()
    }

    pub fn lane(&self, id: ConnectionId) -> Option<Lane> {
        self.lock().lanes.get(&id).cloned()
    }

    /// RPC 入口：capability 未覆盖即回 `NotImplemented`（缺口显式可见，不静默 404）。
    pub fn route(&self, id: ConnectionId, method: &str) -> Route {
        let Some(lane) = self.lane(id) else {
            return Route::UnknownConnection;
        };
        if !lane.capabilities.supports(method) {
            return Route::NotImplemented(method.to_owned());
        }
        Route::Dispatch(method.to_owned())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl Clone for Lane {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            outbound: self.outbound.clone(),
            capabilities: self.capabilities.clone(),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Route {
    Dispatch(String),
    NotImplemented(String),
    UnknownConnection,
}

/// capability 缺口的标准应答（Host 侧统一文案）。
pub fn not_implemented_response(method: &str) -> serde_json::Value {
    serde_json::json!({
        "error": {"code": -32601, "message": format!("Host does not implement `{method}` yet")},
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SUPPORTED: &[&str] = &["settings/read", "v4/conversation/subscribe"];

    #[test]
    fn lanes_follow_arrival_order_not_scheduling_order() {
        let gateway = Gateway::new();
        let (first, _rx1) = gateway.register(Capabilities::default(), 8);
        let (second, _rx2) = gateway.register(Capabilities::default(), 8);
        let (third, _rx3) = gateway.register(Capabilities::default(), 8);
        assert_eq!(first.id.0, 0);
        assert_eq!(second.id.0, 1);
        assert_eq!(third.id.0, 2);
        assert_eq!(gateway.connections(), vec![first.id, second.id, third.id]);
    }

    #[test]
    fn capability_negotiation_drops_unsupported_methods() {
        let capabilities = Capabilities::negotiate(
            &[
                "settings/read".to_owned(),
                "experimental/whatever".to_owned(),
                "v4/conversation/subscribe".to_owned(),
            ],
            SUPPORTED,
        );
        assert!(capabilities.supports("settings/read"));
        assert!(capabilities.supports("v4/conversation/subscribe"));
        assert!(!capabilities.supports("experimental/whatever"));
        assert_eq!(
            capabilities.methods(),
            vec!["settings/read", "v4/conversation/subscribe"]
        );
    }

    #[test]
    fn routing_reports_gaps_explicitly() {
        let gateway = Gateway::new();
        let (lane, _rx) = gateway.register(
            Capabilities::negotiate(&["settings/read".to_owned()], SUPPORTED),
            8,
        );
        assert_eq!(
            gateway.route(lane.id, "settings/read"),
            Route::Dispatch("settings/read".to_owned())
        );
        assert_eq!(
            gateway.route(lane.id, "settings/write"),
            Route::NotImplemented("settings/write".to_owned())
        );
        assert_eq!(
            gateway.route(ConnectionId(99), "settings/read"),
            Route::UnknownConnection
        );
        let response = not_implemented_response("settings/write");
        assert_eq!(response["error"]["code"], -32601);
    }

    #[tokio::test]
    async fn lane_preserves_outbound_order() {
        let gateway = Gateway::new();
        let (lane, mut outbound) = gateway.register(Capabilities::default(), 8);
        for index in 0..3 {
            assert!(lane.outbound.send(serde_json::json!({"seq": index})).await);
        }
        for index in 0..3 {
            assert_eq!(
                outbound.recv().await.unwrap()["seq"],
                index,
                "lane 内顺序 = 入队顺序"
            );
        }
    }

    #[test]
    fn unregister_drops_the_lane() {
        let gateway = Gateway::new();
        let (lane, _rx) = gateway.register(Capabilities::default(), 8);
        assert!(gateway.unregister(lane.id));
        assert!(gateway.connections().is_empty());
        assert_eq!(
            gateway.route(lane.id, "settings/read"),
            Route::UnknownConnection
        );
    }
}
