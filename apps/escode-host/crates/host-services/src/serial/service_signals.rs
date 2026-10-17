//! 服务层的信号与定时循环入口（S4）：由 `mod.rs` 以 `mod service_signals` 引入。

use super::*;

impl SerialService {
    /// TS `setSignals`：未知路径 `notOpen`；与 open/close 在会话内串行。
    pub async fn set_signals(
        &self,
        path: &str,
        dtr: Option<bool>,
        rts: Option<bool>,
        pulse: Option<Pulse>,
    ) -> Result<Signals, SerialOpenError> {
        let session = self.session(path)?;
        session
            .set_signals(dtr, rts, pulse)
            .await
            .map_err(|error| SerialOpenError::new(error.code, error.message))
    }

    /// TS `startLoop`：未知路径 `notOpen`；参数校验在会话内。
    pub fn start_loop(
        &self,
        path: &str,
        bytes: Vec<u8>,
        interval_ms: u64,
        count: Option<u64>,
    ) -> Result<(), SerialOpenError> {
        let session = self.session(path)?;
        session
            .start_loop(bytes, interval_ms, count)
            .map_err(|error| SerialOpenError::new(error.code, error.message))
    }

    /// TS `stopLoop`：未知路径或无循环时都是空操作。
    pub fn stop_loop(&self, path: &str) {
        if let Some(session) = self.lock(&self.inner.sessions).get(path).cloned() {
            session.stop_loop();
        }
    }
}
