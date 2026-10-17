//! 轮询与 `onPorts`（`rust-u2-serial.md` §4/§7）。
use super::*;
use crate::serial::enumerate::{PortEnumerator, PortInfo, port, port_with};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

/// 可编排的枚举器：`ports` 可随时替换，`fail` 控制是否报错，`calls` 记录调用次数。
struct FakeEnumerator {
    ports: Mutex<Vec<PortInfo>>,
    fail: Mutex<bool>,
    calls: AtomicUsize,
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

fn service(fake: &Arc<FakeEnumerator>) -> SerialService {
    SerialService::new(fake.clone()).with_poll_interval(Duration::from_millis(10))
}

fn set_ports(fake: &FakeEnumerator, ports: Vec<PortInfo>) {
    *fake.ports.lock().unwrap() = ports;
}

#[tokio::test]
async fn list_returns_the_enumerated_ports() {
    let fake = Arc::new(FakeEnumerator {
        ports: Mutex::new(vec![port_with("/dev/ttyUSB0", &[("vendorId", "10c4")])]),
        fail: Mutex::new(false),
        calls: AtomicUsize::new(0),
    });
    let ports = service(&fake).list().await.unwrap();
    assert_eq!(ports.len(), 1);
    assert_eq!(ports[0]["path"], serde_json::json!("/dev/ttyUSB0"));
    assert_eq!(ports[0]["vendorId"], serde_json::json!("10c4"));
}

/// `setWatching(true)` 立即轮询并广播一次；端口集合不变时**不重复**广播。
#[tokio::test]
async fn watching_broadcasts_once_and_dedupes_identical_port_sets() {
    let fake = Arc::new(FakeEnumerator {
        ports: Mutex::new(vec![port("/dev/a")]),
        fail: Mutex::new(false),
        calls: AtomicUsize::new(0),
    });
    let service = service(&fake);
    let mut events = service.subscribe_ports();
    service.set_watching(true).await;

    let first = tokio::time::timeout(Duration::from_secs(2), events.recv())
        .await
        .expect("应立刻广播一次")
        .expect("流未关闭");
    assert_eq!(first, serde_json::json!([{"path": "/dev/a"}]));

    // 同样的端口集合：后续轮询不再广播。
    assert!(
        tokio::time::timeout(Duration::from_millis(200), events.recv())
            .await
            .is_err(),
        "端口未变不应重复广播"
    );
    assert!(fake.calls.load(Ordering::SeqCst) > 1, "轮询应持续进行");

    // 端口变化后广播新列表。
    set_ports(&fake, vec![port("/dev/a"), port("/dev/b")]);
    let next = tokio::time::timeout(Duration::from_secs(2), events.recv())
        .await
        .expect("端口变化应广播")
        .expect("流未关闭");
    assert_eq!(
        next,
        serde_json::json!([{"path": "/dev/a"}, {"path": "/dev/b"}])
    );
    service.shutdown();
}

#[tokio::test]
async fn stopping_the_watch_stops_polling() {
    let fake = Arc::new(FakeEnumerator {
        ports: Mutex::new(vec![port("/dev/a")]),
        fail: Mutex::new(false),
        calls: AtomicUsize::new(0),
    });
    let service = service(&fake);
    service.set_watching(true).await;
    tokio::time::sleep(Duration::from_millis(60)).await;
    service.set_watching(false).await;
    tokio::time::sleep(Duration::from_millis(30)).await;
    let settled = fake.calls.load(Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(120)).await;
    assert_eq!(
        fake.calls.load(Ordering::SeqCst),
        settled,
        "关掉 watching 后不应继续轮询"
    );
}

/// 枚举失败：不广播，也不把 dedupe 键写坏——恢复后仍按新值广播。
#[tokio::test]
async fn enumeration_failure_is_silent_and_does_not_poison_dedupe() {
    let fake = Arc::new(FakeEnumerator {
        ports: Mutex::new(vec![port("/dev/a")]),
        fail: Mutex::new(false),
        calls: AtomicUsize::new(0),
    });
    let service = service(&fake);
    let mut events = service.subscribe_ports();
    service.set_watching(true).await;
    events.recv().await.expect("首次广播");

    // 失败期间换端口，再恢复：应广播新列表。
    *fake.fail.lock().unwrap() = true;
    set_ports(&fake, vec![port("/dev/b")]);
    tokio::time::sleep(Duration::from_millis(60)).await;
    assert!(
        tokio::time::timeout(Duration::from_millis(30), events.recv())
            .await
            .is_err(),
        "失败期间不应广播"
    );
    *fake.fail.lock().unwrap() = false;
    let recovered = tokio::time::timeout(Duration::from_secs(2), events.recv())
        .await
        .expect("恢复后应广播")
        .expect("流未关闭");
    assert_eq!(recovered, serde_json::json!([{"path": "/dev/b"}]));
    service.shutdown();
}

/// 「立即轮询」必须与轮询间隔无关：用**长间隔 + 短超时**把两者区分开。
#[tokio::test]
async fn first_poll_happens_immediately_not_after_the_interval() {
    let fake = Arc::new(FakeEnumerator {
        ports: Mutex::new(vec![port("/dev/a")]),
        fail: Mutex::new(false),
        calls: AtomicUsize::new(0),
    });
    // 间隔 5s，但事件必须在 500ms 内到——否则说明实现先睡了一个间隔。
    let service = SerialService::new(fake.clone()).with_poll_interval(Duration::from_secs(5));
    let mut events = service.subscribe_ports();
    service.set_watching(true).await;
    tokio::time::timeout(Duration::from_millis(500), events.recv())
        .await
        .expect("开启 watching 必须立即轮询，而不是等一个间隔")
        .expect("流未关闭");
    service.shutdown();
}
