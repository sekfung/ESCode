//! 私有 broker 监听：Unix socket / Windows 命名管道，每个连接交给 broker 的 serve。
//! node_repl 浏览器 broker（TS `createNodeReplBrowserBroker`）与串口 broker（docs/specs/serial-agent-tools.md）共用。
use super::browser_broker::Broker;
use std::{future::Future, pin::Pin, sync::Arc};
use tokio::io::{AsyncRead, AsyncWrite};

pub(super) trait BrokerConnection: AsyncRead + AsyncWrite + Unpin + Send + 'static {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send + 'static> BrokerConnection for T {}

pub(super) type Serve =
    Arc<dyn Fn(Box<dyn BrokerConnection>) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

pub(super) fn listen(broker: Arc<Broker>) -> std::io::Result<()> {
    let socket = broker.socket.clone();
    listen_socket(&socket, Arc::new(move |stream| Box::pin(broker.clone().serve(stream))))
}

#[cfg(unix)]
pub(super) fn listen_socket(socket: &str, serve: Serve) -> std::io::Result<()> {
    let _ = std::fs::remove_file(socket);
    let listener = tokio::net::UnixListener::bind(socket)?;
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(serve(Box::new(stream)));
        }
    });
    Ok(())
}

#[cfg(windows)]
pub(super) fn listen_socket(socket: &str, serve: Serve) -> std::io::Result<()> {
    use tokio::net::windows::named_pipe::ServerOptions;
    let socket = socket.to_owned();
    let mut server = ServerOptions::new().first_pipe_instance(true).create(&socket)?;
    tokio::spawn(async move {
        loop {
            if server.connect().await.is_err() {
                return;
            }
            // 先建好下一个实例再交出当前连接，避免客户端在两次实例之间连接失败。
            let Ok(next) = ServerOptions::new().create(&socket) else { return };
            let connected = std::mem::replace(&mut server, next);
            tokio::spawn(serve(Box::new(connected)));
        }
    });
    Ok(())
}
