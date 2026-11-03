//! Auto 版本探测预算（docs/specs/rust-mcp-parity.md「协议协商默认值」）。
//!
//! rmcp 3.4 的 Auto 生命周期把 `server/discover` 的等待固定为 10s，且超时后总是回落 legacy；
//! 当前 TS SDK 的预算则是 `min(5s, max(1, floor(timeoutMs / 2)))`，并且只有 stdio 把超时判为
//! legacy，HTTP 超时属于协商失败。这里在 stdio transport 边界复刻 stdio 语义：预算到期时向 rmcp
//! 注入与探测 id 关联的 `-32601` 错误，让它在同一连接上回落 `initialize`；迟到的真实应答被丢弃，
//! 否则会被 `initialize` 的应答读取误判为 id 冲突。

use rmcp::{
    RoleClient,
    model::{
        ClientJsonRpcMessage, ClientRequest, ErrorCode, ErrorData, RequestId, ServerJsonRpcMessage,
    },
    service::RxJsonRpcMessage,
    transport::Transport,
};
use std::{collections::HashSet, future::Future, time::Duration};
use tokio::time::Instant;

pub(super) const MAX_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// TS `resolveVersionNegotiation`：auto 的探测预算。
pub(super) fn timeout(timeout: Duration) -> Duration {
    let millis = (timeout.as_millis() / 2)
        .max(1)
        .min(MAX_PROBE_TIMEOUT.as_millis());
    Duration::from_millis(u64::try_from(millis).unwrap_or(u64::MAX))
}

struct Probe {
    id: RequestId,
    deadline: Instant,
}

/// 只在 Auto 模式使用；预算到期后注入 legacy 错误，并吞掉探测 id 的迟到应答。
pub(super) struct TransportWithProbeBudget<T> {
    inner: T,
    budget: Duration,
    probe: Option<Probe>,
    expired: HashSet<RequestId>,
}

impl<T> TransportWithProbeBudget<T> {
    pub(super) fn new(inner: T, budget: Duration) -> Self {
        Self {
            inner,
            budget,
            probe: None,
            expired: HashSet::new(),
        }
    }
}

fn discover_id(message: &ClientJsonRpcMessage) -> Option<RequestId> {
    match message {
        ClientJsonRpcMessage::Request(request)
            if matches!(request.request, ClientRequest::DiscoverRequest(_)) =>
        {
            Some(request.id.clone())
        }
        _ => None,
    }
}

fn reply_id(message: &ServerJsonRpcMessage) -> Option<&RequestId> {
    match message {
        ServerJsonRpcMessage::Response(response) => Some(&response.id),
        ServerJsonRpcMessage::Error(error) => error.id.as_ref(),
        _ => None,
    }
}

impl<T> Transport<RoleClient> for TransportWithProbeBudget<T>
where
    T: Transport<RoleClient>,
{
    type Error = T::Error;

    fn send(
        &mut self,
        item: ClientJsonRpcMessage,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send + 'static {
        if let Some(id) = discover_id(&item) {
            self.probe = Some(Probe {
                id,
                deadline: Instant::now() + self.budget,
            });
        }
        self.inner.send(item)
    }

    fn receive(&mut self) -> impl Future<Output = Option<RxJsonRpcMessage<RoleClient>>> + Send {
        async move {
            loop {
                let active = self
                    .probe
                    .as_ref()
                    .map(|probe| (probe.id.clone(), probe.deadline));
                let message = match active {
                    Some((_, deadline)) => {
                        tokio::select! {
                            biased;
                            message = self.inner.receive() => message,
                            _ = tokio::time::sleep_until(deadline) => {
                                let probe = self.probe.take().expect("active probe");
                                self.expired.insert(probe.id.clone());
                                return Some(ServerJsonRpcMessage::error(
                                    ErrorData::new(
                                        ErrorCode::METHOD_NOT_FOUND,
                                        "server/discover probe timed out",
                                        None,
                                    ),
                                    Some(probe.id),
                                ));
                            }
                        }
                    }
                    None => self.inner.receive().await,
                };
                let Some(message) = message else { return None };
                if let Some(id) = reply_id(&message) {
                    if self.expired.remove(id) {
                        continue;
                    }
                    if self.probe.as_ref().is_some_and(|probe| &probe.id == id) {
                        self.probe = None;
                        return Some(message);
                    }
                }
                return Some(message);
            }
        }
    }

    fn close(&mut self) -> impl Future<Output = Result<(), Self::Error>> + Send {
        self.inner.close()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::model::{DiscoverRequest, DiscoverRequestParams};
    use tokio::sync::mpsc;

    struct FakeTransport {
        incoming: mpsc::UnboundedReceiver<ServerJsonRpcMessage>,
    }

    impl Transport<RoleClient> for FakeTransport {
        type Error = std::io::Error;

        fn send(
            &mut self,
            _item: ClientJsonRpcMessage,
        ) -> impl Future<Output = Result<(), Self::Error>> + Send + 'static {
            async { Ok(()) }
        }

        fn receive(&mut self) -> impl Future<Output = Option<RxJsonRpcMessage<RoleClient>>> + Send {
            async move { self.incoming.recv().await }
        }

        fn close(&mut self) -> impl Future<Output = Result<(), Self::Error>> + Send {
            async { Ok(()) }
        }
    }

    fn probe_request(id: RequestId) -> ClientJsonRpcMessage {
        ClientJsonRpcMessage::request(
            ClientRequest::DiscoverRequest(DiscoverRequest::new(DiscoverRequestParams {})),
            id,
        )
    }

    #[test]
    fn budget_matches_ts_formula() {
        assert_eq!(timeout(Duration::from_millis(30_000)), MAX_PROBE_TIMEOUT);
        assert_eq!(
            timeout(Duration::from_millis(4_000)),
            Duration::from_secs(2)
        );
        assert_eq!(timeout(Duration::from_millis(3)), Duration::from_millis(1));
        assert_eq!(timeout(Duration::ZERO), Duration::from_millis(1));
    }

    #[tokio::test(start_paused = true)]
    async fn expired_probe_falls_back_and_drops_late_reply() {
        let (tx, rx) = mpsc::unbounded_channel();
        let mut transport = TransportWithProbeBudget::new(
            FakeTransport { incoming: rx },
            Duration::from_millis(100),
        );
        let probe_id = RequestId::Number(1);
        transport
            .send(probe_request(probe_id.clone()))
            .await
            .unwrap();

        // 预算到期：包装层返回与探测 id 关联的 legacy 错误，rmcp 据此回落 initialize。
        let timed_out = tokio::time::timeout(Duration::from_millis(150), transport.receive())
            .await
            .expect("budget should expire")
            .expect("synthetic reply");
        match timed_out {
            ServerJsonRpcMessage::Error(error) => {
                assert_eq!(error.id, Some(probe_id.clone()));
                assert_eq!(error.error.code, ErrorCode::METHOD_NOT_FOUND);
            }
            other => panic!("unexpected message: {other:?}"),
        }

        // 真实 server 迟到的探测应答必须被丢弃，随后 initialize 应答正常透出。
        tx.send(ServerJsonRpcMessage::error(
            ErrorData::new(ErrorCode::METHOD_NOT_FOUND, "late", None),
            Some(probe_id),
        ))
        .unwrap();
        let initialize_id = RequestId::Number(2);
        tx.send(ServerJsonRpcMessage::error(
            ErrorData::new(ErrorCode::INTERNAL_ERROR, "initialize", None),
            Some(initialize_id.clone()),
        ))
        .unwrap();
        let next = transport.receive().await.expect("initialize reply");
        match next {
            ServerJsonRpcMessage::Error(error) => assert_eq!(error.id, Some(initialize_id)),
            other => panic!("unexpected message: {other:?}"),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn reply_within_budget_is_forwarded_unchanged() {
        let (tx, rx) = mpsc::unbounded_channel();
        let mut transport = TransportWithProbeBudget::new(
            FakeTransport { incoming: rx },
            Duration::from_millis(100),
        );
        let probe_id = RequestId::Number(1);
        transport
            .send(probe_request(probe_id.clone()))
            .await
            .unwrap();
        tx.send(ServerJsonRpcMessage::error(
            ErrorData::new(ErrorCode::METHOD_NOT_FOUND, "legacy", None),
            Some(probe_id.clone()),
        ))
        .unwrap();

        let reply = transport.receive().await.expect("probe reply");
        match reply {
            ServerJsonRpcMessage::Error(error) => assert_eq!(error.id, Some(probe_id)),
            other => panic!("unexpected message: {other:?}"),
        }

        // 正常应答后探测状态清空，后续消息不能被误吞。
        let next_id = RequestId::Number(9);
        tx.send(ServerJsonRpcMessage::error(
            ErrorData::new(ErrorCode::INTERNAL_ERROR, "next", None),
            Some(next_id.clone()),
        ))
        .unwrap();
        let next = transport.receive().await.expect("next reply");
        match next {
            ServerJsonRpcMessage::Error(error) => assert_eq!(error.id, Some(next_id)),
            other => panic!("unexpected message: {other:?}"),
        }
    }
}
