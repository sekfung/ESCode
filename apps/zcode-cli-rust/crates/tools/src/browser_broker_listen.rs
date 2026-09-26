//! broker 监听：Unix socket / Windows 命名管道，每个连接交给 `Broker::serve`（TS `createNodeReplBrowserBroker`）。
use super::browser_broker::Broker;
use std::sync::Arc;

#[cfg(unix)]
pub(super) fn listen(broker: Arc<Broker>) -> std::io::Result<()> {
    let _ = std::fs::remove_file(&broker.socket);
    let listener = tokio::net::UnixListener::bind(&broker.socket)?;
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(broker.clone().serve(stream));
        }
    });
    Ok(())
}

#[cfg(windows)]
pub(super) fn listen(broker: Arc<Broker>) -> std::io::Result<()> {
    use tokio::net::windows::named_pipe::ServerOptions;
    let mut server = ServerOptions::new().first_pipe_instance(true).create(&broker.socket)?;
    tokio::spawn(async move {
        loop {
            if server.connect().await.is_err() {
                return;
            }
            // 先建好下一个实例再交出当前连接，避免客户端在两次实例之间连接失败。
            let Ok(next) = ServerOptions::new().create(&broker.socket) else { return };
            let connected = std::mem::replace(&mut server, next);
            tokio::spawn(broker.clone().serve(connected));
        }
    });
    Ok(())
}
