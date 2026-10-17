//! 测试用假传输：可编排「打开失败」与打开/关闭计数。

use super::{SerialConfig, SerialOpenError, SerialPort, SerialTransport};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Default)]
pub struct FakeTransport {
    pub opened: Mutex<Vec<String>>,
    pub closed: std::sync::Arc<AtomicUsize>,
    pub fail: Mutex<Option<SerialOpenError>>,
}

struct FakePort {
    closed: std::sync::Arc<AtomicUsize>,
}

impl SerialPort for FakePort {
    fn close(&self) {
        self.closed.fetch_add(1, Ordering::SeqCst);
    }
}

#[async_trait::async_trait]
impl SerialTransport for FakeTransport {
    async fn open(
        &self,
        path: &str,
        _config: &SerialConfig,
    ) -> Result<Box<dyn SerialPort>, SerialOpenError> {
        if let Some(error) = self.fail.lock().unwrap().clone() {
            return Err(error);
        }
        self.opened.lock().unwrap().push(path.to_owned());
        Ok(Box::new(FakePort {
            closed: std::sync::Arc::clone(&self.closed),
        }))
    }
}
