//! 测试用假传输：可编排打开失败、记录写入、并向读循环喂字节。
use super::{SerialConfig, SerialIoError, SerialOpenError, SerialPort, SerialTransport};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

type Sink = Arc<dyn Fn(Vec<u8>) + Send + Sync>;

#[derive(Default)]
pub struct FakeTransport {
    pub opened: Mutex<Vec<String>>,
    pub closed: Arc<AtomicUsize>,
    pub writes: Arc<Mutex<Vec<Vec<u8>>>>,
    pub fail: Mutex<Option<SerialOpenError>>,
    /// 最近一次 `spawn_reader` 注册的上送回调（测试用它模拟设备发数据）。
    pub sink: Arc<Mutex<Option<Sink>>>,
}

impl FakeTransport {
    /// 模拟设备侧发来字节。
    pub fn feed(&self, bytes: &[u8]) {
        let sink = self.sink.lock().unwrap().clone();
        if let Some(sink) = sink {
            sink(bytes.to_vec());
        }
    }
}

struct FakePort {
    closed: Arc<AtomicUsize>,
    writes: Arc<Mutex<Vec<Vec<u8>>>>,
    sink: Arc<Mutex<Option<Sink>>>,
}

#[async_trait::async_trait]
impl SerialPort for FakePort {
    fn close(&self) {
        self.closed.fetch_add(1, Ordering::SeqCst);
    }

    async fn write(&self, bytes: Vec<u8>) -> Result<(), SerialIoError> {
        self.writes.lock().unwrap().push(bytes);
        Ok(())
    }

    fn spawn_reader(&self, sink: Sink) {
        *self.sink.lock().unwrap() = Some(sink);
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
            closed: Arc::clone(&self.closed),
            writes: Arc::clone(&self.writes),
            sink: Arc::clone(&self.sink),
        }))
    }
}
