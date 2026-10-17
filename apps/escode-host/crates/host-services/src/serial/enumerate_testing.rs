//! 测试用假枚举器：可随时替换端口集合、可编排失败。
use super::enumerate::{PortEnumerator, PortInfo};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// 可编排的枚举器：`set_ports` 替换端口集合，`fail(true)` 让 `list` 报错。
pub struct FakeEnumerator {
    ports: Mutex<Vec<PortInfo>>,
    fail: Mutex<bool>,
    pub calls: AtomicUsize,
}

impl FakeEnumerator {
    pub fn new(ports: Vec<PortInfo>) -> Arc<Self> {
        Arc::new(Self {
            ports: Mutex::new(ports),
            fail: Mutex::new(false),
            calls: AtomicUsize::new(0),
        })
    }

    pub fn set_ports(&self, ports: Vec<PortInfo>) {
        *self.ports.lock().unwrap() = ports;
    }

    pub fn fail(&self, fail: bool) {
        *self.fail.lock().unwrap() = fail;
    }
}

#[async_trait::async_trait]
impl PortEnumerator for FakeEnumerator {
    async fn list(&self) -> Result<Vec<PortInfo>, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if *self.fail.lock().unwrap() {
            return Err("boom".to_owned());
        }
        Ok(self.ports.lock().unwrap().clone())
    }
}
