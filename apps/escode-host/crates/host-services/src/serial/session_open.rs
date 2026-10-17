//! 会话的打开/关闭/重连（S2）：由 `session.rs` 以 `mod open` 引入。
//!
//! open/close/reconnect 在会话内串行；用户主动打开重置信号，自动重连保留断开前的状态。

use super::*;

impl SerialSession {
    /// TS `open`：已在打开同一配置时不重复打开；状态为 `disconnected` 时先收口再开。
    pub async fn open(&self, config: SerialConfig) -> Result<(), SerialOpenError> {
        let _guard = self.queue.lock().await;
        let already_open = {
            let status = self.lock(&self.status);
            status.state == "open" && status.config.as_ref() == Some(&config)
        };
        if already_open {
            return Ok(());
        }
        if self.lock(&self.port).is_some() || self.state() == "disconnected" {
            self.close_now();
        }
        // 用户主动打开时信号回到默认值；自动重连（reconnect）则保留断开前的状态。
        *self.lock(&self.signals) = DEFAULT_SIGNALS;
        self.open_port(config).await
    }

    /// TS `close`：幂等；关闭后状态回到 `closed`（保留 config 便于展示）。
    pub async fn close(&self) {
        let _guard = self.queue.lock().await;
        self.close_now();
    }

    /// 设备重新出现时由轮询调用：只有仍处于 `disconnected` 才重连。
    pub async fn reconnect(&self) -> Result<(), SerialOpenError> {
        let _guard = self.queue.lock().await;
        let (state, config) = {
            let status = self.lock(&self.status);
            (status.state, status.config.clone())
        };
        if state != "disconnected" {
            return Ok(());
        }
        let Some(config) = config else {
            return Ok(());
        };
        self.open_port(config).await
    }

    /// 设备消失（由轮询发现）：`autoReconnect` 为真进 `disconnected`，否则直接 `closed`。
    pub async fn handle_disconnect(&self) {
        let _guard = self.queue.lock().await;
        let config = self.lock(&self.status).config.clone();
        self.data.shutdown();
        if let Some(port) = self.lock(&self.port).take() {
            port.close();
        }
        let auto_reconnect = config.as_ref().is_some_and(|config| config.auto_reconnect);
        self.set_status(
            if auto_reconnect {
                "disconnected"
            } else {
                "closed"
            },
            config,
            None,
        );
    }

    async fn open_port(&self, config: SerialConfig) -> Result<(), SerialOpenError> {
        self.set_status("opening", Some(config.clone()), None);
        match self.transport.open(&self.path, &config).await {
            Ok(port) => {
                let port: Arc<dyn SerialPort> = Arc::from(port);
                let data = Arc::clone(&self.data);
                port.spawn_reader(Arc::new(move |bytes: Vec<u8>| data.receive(bytes)));
                *self.lock(&self.port) = Some(Arc::clone(&port));
                // 自动重连：恢复断开前的信号；失败不影响 open（TS 记 warn）。
                let signals = *self.lock(&self.signals);
                if signals != DEFAULT_SIGNALS {
                    let _ = port.set_signals(signals).await;
                }
                self.set_status("open", Some(config), None);
                Ok(())
            }
            Err(error) => {
                let mapped = if error.code == "io" {
                    SerialOpenError::from_message(error.message)
                } else {
                    error
                };
                self.set_status(
                    "error",
                    Some(config),
                    Some((mapped.code.to_owned(), mapped.message.clone())),
                );
                Err(mapped)
            }
        }
    }

    fn close_now(&self) {
        let port = self.lock(&self.port).take();
        let config = self.lock(&self.status).config.clone();
        if port.is_some() {
            // 关闭前停读循环并收口未成段的 RX（TS `buffer.flushRx()`）。
            self.data.shutdown();
            self.set_status("closing", config.clone(), None);
            if let Some(port) = port {
                port.close();
            }
            self.set_status("closed", config, None);
            return;
        }
        if self.state() != "closed" {
            self.set_status("closed", config, None);
        }
    }
}
