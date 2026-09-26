//! 跨日 reminder（docs/specs/rust-date-change.md，对齐 TS `injectDateChangeReminderIntoMessageHistory`）：
//! 同一运行时内首轮只记录本地日期；日期变化后的下一轮在用户输入前插入 reminder，同日不重复。
use anyhow::Result;
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
mod support;
use support::answer_runtime_preferences;
use zcode_cli_rust::{app::Engine, contract::*, domain::session::Session};

struct Store;
#[async_trait]
impl SessionStore for Store {
    async fn load_index(&self, _: &str) -> Result<BTreeMap<String, Value>> {
        Ok(BTreeMap::new())
    }
    async fn lookup_ack(&self, _: &str, _: &str) -> Result<Option<Value>> {
        Ok(None)
    }
    async fn load(&self, _: &str) -> Result<(Vec<Session>, BTreeMap<String, Value>)> {
        Ok((vec![], BTreeMap::new()))
    }
    async fn load_session(&self, _: &str, _: &str) -> Result<Option<Session>> {
        Ok(Some(Session::new(
            "session".into(),
            "workspace".into(),
            "p".into(),
            "m".into(),
            "none".into(),
            "epoch".into(),
            1,
        )))
    }
    async fn commit(
        &self,
        _: &str,
        _: Option<&mut Session>,
        _: Option<(String, Value)>,
    ) -> Result<()> {
        Ok(())
    }
}
struct Model(mpsc::UnboundedSender<Vec<Value>>);
#[async_trait]
impl ModelPort for Model {
    async fn complete(
        &self,
        messages: Vec<Value>,
        _: &[Value],
        _: &EventSink,
        _: &CancellationToken,
    ) -> std::result::Result<ModelOutput, ModelFailure> {
        self.0.send(messages).unwrap();
        Ok(ModelOutput {
            response_id: String::new(),
            output_limit: false,
            message: json!({"role":"assistant","content":"ok"}),
            calls: vec![],
            usage: json!({"prompt_tokens":1,"completion_tokens":1}),
        })
    }
}
struct Tools;
#[async_trait]
impl ToolPort for Tools {
    fn definitions(&self) -> Vec<Value> {
        vec![]
    }
    fn requires_permission(&self, _: &str) -> bool {
        false
    }
    async fn execute(&self, _: &str, _: &Value, _: &CancellationToken) -> Result<String> {
        panic!("no tool expected")
    }
}
struct Clock {
    ids: AtomicUsize,
    date: Arc<Mutex<String>>,
}
impl RuntimeClock for Clock {
    fn id(&self) -> String {
        format!("id{}", self.ids.fetch_add(1, Ordering::SeqCst))
    }
    fn now(&self) -> u64 {
        1000
    }
    fn local_date(&self) -> Option<String> {
        Some(self.date.lock().unwrap().clone())
    }
}

fn reminders(messages: &[Value]) -> Vec<String> {
    messages
        .iter()
        .filter_map(|m| m["content"].as_str())
        .filter(|c| c.contains("The date has changed."))
        .map(str::to_owned)
        .collect()
}

#[tokio::test]
async fn date_change_reminder_is_injected_once_after_the_local_date_rolls_over() {
    let date = Arc::new(Mutex::new("2026-09-26".to_owned()));
    let (requests_tx, mut requests) = mpsc::unbounded_channel();
    let engine = Engine::new(
        "workspace".into(),
        Some(ModelIdentity {
            provider_id: "p".into(),
            model_id: "m".into(),
            reasoning_level: "none".into(),
        }),
        RuntimePorts {
            context: Arc::new(zcode_cli_host::context_source::WorkspaceContext::new(
                std::env::temp_dir(),
                std::env::temp_dir().join("fixture-empty-home"),
                false,
            )),
            store: Arc::new(Store),
            model: Some(Arc::new(Model(requests_tx))),
            tools: Arc::new(Tools),
            clock: Arc::new(Clock {
                ids: AtomicUsize::new(1),
                date: date.clone(),
            }),
        },
    )
    .await
    .unwrap();
    let (tx, rx) = mpsc::channel(8);
    let (out, raw_output) = mpsc::channel(32);
    let mut output = answer_runtime_preferences(raw_output, tx.clone());
    let running = tokio::spawn(engine.serve(rx, out, CancellationToken::new()));
    let drain = tokio::spawn(async move { while output.recv().await.is_some() {} });
    // 短输入（<10 字符）不触发标题 sidecar，模型只收到主循环请求。
    let mut send = async |n: usize, text: &str| {
        tx.send(Input::Request(serde_json::from_value(json!({"id":n,"method":"v4/command","params":{"commandId":format!("c{n}"),"clientId":"test","sessionId":"session","type":"sendText","issuedAt":1,"payload":{"text":text}}})).unwrap()))
            .await
            .unwrap();
        let request = tokio::time::timeout(Duration::from_secs(3), requests.recv())
            .await
            .unwrap()
            .unwrap();
        // 等本轮收口再改日期，下一条输入以新的 admission 进入。
        tokio::time::sleep(Duration::from_millis(200)).await;
        request
    };
    let first = send(1, "one").await;
    assert!(
        reminders(&first).is_empty(),
        "first turn only records the date"
    );
    let same_day = send(2, "two").await;
    assert!(reminders(&same_day).is_empty(), "same day never reminds");
    *date.lock().unwrap() = "2026-09-27".into();
    let next_day = send(3, "three").await;
    assert_eq!(
        reminders(&next_day),
        vec![
            "<system-reminder>\nThe date has changed. Today's date is now 2026-09-27. DO NOT mention this to the user explicitly because they are already aware.\n</system-reminder>"
        ]
    );
    // reminder 位于本轮用户输入之前（TS 注入顺序），并保留在历史中、不重复追加。
    let user = next_day
        .iter()
        .rposition(|m| m["content"] == "three")
        .unwrap();
    assert!(
        next_day[user - 1]["content"]
            .as_str()
            .unwrap()
            .contains("The date has changed.")
    );
    let later = send(4, "four").await;
    assert_eq!(reminders(&later).len(), 1);
    tx.send(Input::Eof).await.unwrap();
    tokio::time::timeout(Duration::from_secs(3), running)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    drain.await.unwrap();
}
