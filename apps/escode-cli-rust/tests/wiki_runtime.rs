//! LLM Wiki × 真实 Engine（apps/escode-cli-rust/docs/llm-wiki/01-spec.md §9、验收 §15.2）：
//! 用真实 Engine 与真实 WikiService（临时 HOME / 项目），模型桩记录每次请求的消息与工具定义，
//! 证明知识确实进入模型上下文、工具真实可调用、未审核知识不注入、关闭/出错时原流程不受影响。
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
use escode_cli_rust::{app::Engine, contract::*, domain::session::Session};
use escode_cli_wiki::WikiService;
use support::answer_runtime_preferences;

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
        Ok(Some(Session::new("session".into(), "workspace".into(), "p".into(), "m".into(), "none".into(), "epoch".into(), 1)))
    }
    async fn commit(&self, _: &str, _: Option<&mut Session>, _: Option<(String, Value)>) -> Result<()> {
        Ok(())
    }
}

/// 模型桩：记录 (消息, 工具定义)；脚本里排队的工具调用依次返回，没有则直接回答。
struct Model {
    requests: mpsc::UnboundedSender<(Vec<Value>, Vec<Value>)>,
    script: Mutex<Vec<Value>>,
}
#[async_trait]
impl ModelPort for Model {
    async fn complete(&self, messages: Vec<Value>, tools: &[Value], _: &EventSink, _: &CancellationToken) -> std::result::Result<ModelOutput, ModelFailure> {
        self.requests.send((messages, tools.to_vec())).unwrap();
        let next = self.script.lock().unwrap().pop();
        let (message, calls) = match next {
            Some(call) => (json!({"role": "assistant", "content": null, "tool_calls": [call.clone()]}), vec![call]),
            None => (json!({"role": "assistant", "content": "ok"}), vec![]),
        };
        Ok(ModelOutput { response_id: String::new(), output_limit: false, message, calls, usage: json!({"prompt_tokens": 1, "completion_tokens": 1}) })
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
        panic!("no workspace tool expected")
    }
}

struct Clock(AtomicUsize);
impl RuntimeClock for Clock {
    fn id(&self) -> String {
        format!("id{}", self.0.fetch_add(1, Ordering::SeqCst))
    }
    fn now(&self) -> u64 {
        1000
    }
}

struct Fixture {
    _home: tempfile::TempDir,
    _project: tempfile::TempDir,
    wiki: Arc<WikiService>,
}

/// 临时项目 + 已批准的项目规范 + 一条未审核提案。
async fn fixture() -> Fixture {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(project.path().join(".git")).unwrap();
    std::fs::write(project.path().join(".git/config"), "[remote \"origin\"]\n\turl = https://h/o/runtime\n").unwrap();
    let doc = project.path().join("docs/standards.md");
    std::fs::create_dir_all(doc.parent().unwrap()).unwrap();
    std::fs::write(&doc, "# Shop Standards\n\n## Pagination\n\nList endpoints must use cursor pagination; offset pagination is forbidden.\n").unwrap();
    let wiki = Arc::new(WikiService::new(project.path().to_owned(), None, home.path().to_owned()));
    wiki.ingest(&[doc], escode_cli_rust::domain::wiki::entry::Scope::Project, true).await.unwrap();
    let proposal = escode_cli_rust::domain::wiki::tools::ProposeInput {
        title: "Pagination page size proposal".into(),
        body: "List endpoints should cap pagination page size at 50 items.".into(),
        kind: "standard".into(),
        rationale: "draft".into(),
        scope: None,
        tags: None,
        applies_to_paths: None,
        evidence: None,
    };
    wiki.propose(&proposal, "seed").await.unwrap();
    Fixture { _home: home, _project: project, wiki }
}

type Requests = mpsc::UnboundedReceiver<(Vec<Value>, Vec<Value>)>;

async fn start(wiki: Option<Arc<dyn WikiPort>>, script: Vec<Value>) -> (mpsc::Sender<Input>, Requests, tokio::task::JoinHandle<Result<()>>) {
    let (tx, requests, running, mut output) = start_with_output(wiki, script).await;
    tokio::spawn(async move { while output.recv().await.is_some() {} });
    (tx, requests, running)
}

async fn start_with_output(wiki: Option<Arc<dyn WikiPort>>, script: Vec<Value>) -> (mpsc::Sender<Input>, Requests, tokio::task::JoinHandle<Result<()>>, mpsc::Receiver<Vec<Value>>) {
    let (requests_tx, requests) = mpsc::unbounded_channel();
    let mut engine = Engine::new(
        "workspace".into(),
        Some(ModelIdentity { provider_id: "p".into(), model_id: "m".into(), reasoning_level: "none".into() }),
        RuntimePorts {
            context: Arc::new(escode_cli_host::context_source::WorkspaceContext::new(std::env::temp_dir(), std::env::temp_dir().join("fixture-empty-home"), false)),
            store: Arc::new(Store),
            model: Some(Arc::new(Model { requests: requests_tx, script: Mutex::new(script) })),
            tools: Arc::new(Tools),
            clock: Arc::new(Clock(AtomicUsize::new(1))),
        },
    )
    .await
    .unwrap();
    if let Some(wiki) = wiki {
        engine = engine.with_wiki(wiki);
    }
    let (tx, rx) = mpsc::channel(8);
    let (out, raw_output) = mpsc::channel(32);
    let output = answer_runtime_preferences(raw_output, tx.clone());
    let running = tokio::spawn(engine.serve(rx, out, CancellationToken::new()));
    (tx, requests, running, output)
}

async fn send(tx: &mpsc::Sender<Input>, n: usize, text: &str) {
    tx.send(Input::Request(serde_json::from_value(json!({"id": n, "method": "v4/command", "params": {"commandId": format!("c{n}"), "clientId": "test", "sessionId": "session", "type": "sendText", "issuedAt": 1, "payload": {"text": text}}})).unwrap()))
        .await
        .unwrap();
}

async fn next(requests: &mut Requests) -> (Vec<Value>, Vec<Value>) {
    tokio::time::timeout(Duration::from_secs(5), requests.recv()).await.unwrap().unwrap()
}

/// 主循环请求：最后一条 user 消息就是本轮输入（跳过首条长输入触发的会话标题辅助请求）。
async fn next_turn(requests: &mut Requests, text: &str) -> (Vec<Value>, Vec<Value>) {
    loop {
        let (messages, tools) = next(requests).await;
        let last_user = messages.iter().rev().find(|m| m["role"] == "user").and_then(|m| m["content"].as_str()).unwrap_or_default();
        if last_user == text {
            return (messages, tools);
        }
    }
}

fn wiki_messages(messages: &[Value]) -> Vec<&str> {
    messages.iter().filter(|m| m["content"].as_str().is_some_and(|c| c.contains("ESCode LLM Wiki"))).filter_map(|m| m["content"].as_str()).collect()
}

fn tool_names(tools: &[Value]) -> Vec<&str> {
    tools.iter().filter_map(|t| t["function"]["name"].as_str()).collect()
}

async fn stop(tx: mpsc::Sender<Input>, running: tokio::task::JoinHandle<Result<()>>) {
    tx.send(Input::Eof).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), running).await.unwrap().unwrap().unwrap();
}

#[tokio::test]
async fn coding_turn_injects_approved_knowledge_before_the_user_input() {
    let f = fixture().await;
    let (tx, mut requests, running) = start(Some(f.wiki.clone()), vec![]).await;
    send(&tx, 1, "implement the orders list endpoint with pagination").await;
    let (messages, tools) = next_turn(&mut requests, "implement the orders list endpoint with pagination").await;
    let injected = wiki_messages(&messages);
    assert_eq!(injected.len(), 1, "{messages:#?}");
    assert!(injected[0].starts_with("<system-reminder>"));
    assert!(injected[0].contains("cursor pagination") && injected[0].contains("[wiki:<id>]"));
    assert!(!injected[0].contains("page size at 50"), "unreviewed proposal is never injected");
    let user = messages.iter().rposition(|m| m["content"].as_str().is_some_and(|c| c.contains("orders list endpoint"))).unwrap();
    assert!(messages[user - 1]["content"].as_str().unwrap().contains("ESCode LLM Wiki"), "wiki context sits right before the user input");
    for name in ["WikiSearch", "WikiRead", "WikiPropose", "WikiManage"] {
        assert!(tool_names(&tools).contains(&name), "missing {name}");
    }
    // 同一会话再次问相关问题：同版本条目已在上下文中，不重复注入。
    tokio::time::sleep(Duration::from_millis(200)).await;
    send(&tx, 2, "also paginate the customers list endpoint").await;
    let (messages, _) = next_turn(&mut requests, "also paginate the customers list endpoint").await;
    assert_eq!(wiki_messages(&messages).len(), 1, "no duplicate injection");
    stop(tx, running).await;
    let audit = f.wiki.audit_tail(20).await.unwrap();
    assert!(audit.iter().any(|e| e["event"] == "retrieval" && e["status"] == "injected" && e["session"] == "session"));
}

#[tokio::test]
async fn review_turn_gets_review_protocol() {
    let f = fixture().await;
    let (tx, mut requests, running) = start(Some(f.wiki.clone()), vec![]).await;
    send(&tx, 1, "please review my pagination diff for the list endpoint").await;
    let (messages, _) = next(&mut requests).await;
    let injected = wiki_messages(&messages);
    assert!(injected[0].contains("for this review task") && injected[0].contains("Review protocol"));
    stop(tx, running).await;
}

#[tokio::test]
async fn model_can_call_wiki_tools_in_the_real_loop() {
    let f = fixture().await;
    let call = json!({"id": "call_1", "type": "function", "function": {"name": "WikiSearch", "arguments": "{\"query\":\"cursor pagination\"}"}});
    let (tx, mut requests, running) = start(Some(f.wiki.clone()), vec![call]).await;
    send(&tx, 1, "hello there friend").await;
    let (first, _) = next(&mut requests).await;
    assert!(wiki_messages(&first).is_empty(), "general chit-chat below threshold is not injected");
    let (second, _) = next(&mut requests).await;
    let result = second.iter().rev().find(|m| m["role"] == "tool").expect("tool result in the next request");
    let content = result["content"].as_str().unwrap();
    assert!(content.contains("<wiki-search") && content.contains("cursor pagination"), "{content}");
    stop(tx, running).await;
}

#[tokio::test]
async fn without_wiki_or_on_failure_the_turn_is_unchanged() {
    // 未组合 Wiki：无工具、无注入。
    let (tx, mut requests, running) = start(None, vec![]).await;
    send(&tx, 1, "implement the orders list endpoint with pagination").await;
    let (messages, tools) = next(&mut requests).await;
    assert!(wiki_messages(&messages).is_empty());
    assert!(!tool_names(&tools).iter().any(|n| n.starts_with("Wiki")));
    stop(tx, running).await;
    // 端口出错：fail-open，本轮照常请求模型。
    struct Broken;
    #[async_trait]
    impl WikiPort for Broken {
        async fn surface(&self) -> WikiSurface {
            WikiSurface { enabled: true, tools: false, auto_inject: true }
        }
        async fn retrieve_for_turn(&self, _: WikiTurnInput, _: &CancellationToken) -> Result<WikiTurnRetrieval> {
            anyhow::bail!("disk unavailable")
        }
        async fn tool(&self, _: &str, _: &Value, _: &str) -> Result<WikiToolResult> {
            anyhow::bail!("unused")
        }
        async fn request(&self, _: &str, _: &Value) -> Result<Value> {
            anyhow::bail!("unused")
        }
    }
    let (tx, mut requests, running) = start(Some(Arc::new(Broken)), vec![]).await;
    send(&tx, 1, "implement the orders list endpoint with pagination").await;
    let (messages, _) = next(&mut requests).await;
    assert!(wiki_messages(&messages).is_empty());
    assert!(messages.iter().any(|m| m["content"].as_str().is_some_and(|c| c.contains("orders list endpoint"))));
    stop(tx, running).await;
}

/// 等到指定 id 的 RPC 响应（Engine 经 AuxiliaryReply 异步回复）。
async fn response(output: &mut mpsc::Receiver<Vec<Value>>, id: u64) -> Value {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let batch = output.recv().await.expect("engine output closed");
            if let Some(found) = batch.into_iter().find(|m| m["id"] == id) {
                return found;
            }
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn desktop_protocol_is_served_through_the_engine() {
    let f = fixture().await;
    let (tx, _requests, running, mut output) = start_with_output(Some(f.wiki.clone()), vec![]).await;
    let request = |id: u64, method: &str, params: Value| Input::Request(serde_json::from_value(json!({"id": id, "method": method, "params": params})).unwrap());
    tx.send(request(7, "v4/wiki/status", json!({}))).await.unwrap();
    let status = response(&mut output, 7).await;
    assert_eq!(status["result"]["supported"], true, "{status}");
    assert_eq!(status["result"]["project"]["identityKind"], "git-remote");
    tx.send(request(8, "v4/wiki/graph", json!({"includeSources": true}))).await.unwrap();
    let graph = response(&mut output, 8).await;
    let nodes = graph["result"]["nodes"].as_array().unwrap();
    assert!(nodes.iter().any(|n| n["type"] == "source") && nodes.iter().any(|n| n["status"] == "proposal"));
    assert!(graph["result"]["edges"].as_array().unwrap().iter().any(|e| e["type"] == "source"));
    let proposal = nodes.iter().find(|n| n["status"] == "proposal").unwrap()["entryId"].as_str().unwrap().to_owned();
    tx.send(request(9, "v4/wiki/entry", json!({"id": proposal}))).await.unwrap();
    assert_eq!(response(&mut output, 9).await["result"]["entry"]["status"], "proposal");
    // Desktop 的批准是用户显式操作：经同一治理路径，审计记录发起方为 desktop。
    tx.send(request(10, "v4/wiki/manage", json!({"action": "approve", "ids": [proposal.clone()]}))).await.unwrap();
    assert_eq!(response(&mut output, 10).await["result"]["ok"], true);
    assert!(f.wiki.audit_tail(5).await.unwrap().iter().any(|e| e["event"] == "approve" && e["by"] == "desktop"));
    tx.send(request(11, "v4/wiki/entry", json!({}))).await.unwrap();
    assert!(response(&mut output, 11).await["error"].is_object(), "missing id is an RPC error");
    stop(tx, running).await;
    // 未组合 Wiki 的 runtime：status 明确 supported=false，其余方法报错。
    let (tx, _requests, running, mut output) = start_with_output(None, vec![]).await;
    tx.send(request(1, "v4/wiki/status", json!({}))).await.unwrap();
    assert_eq!(response(&mut output, 1).await["result"]["supported"], false);
    tx.send(request(2, "v4/wiki/graph", json!({}))).await.unwrap();
    assert!(response(&mut output, 2).await["error"]["message"].as_str().unwrap().contains("wiki_unavailable"));
    stop(tx, running).await;
}

#[tokio::test]
async fn wiki_manage_always_waits_for_user_approval_even_in_yolo() {
    let f = fixture().await;
    let snapshot = f.wiki.graph(&json!({})).await.unwrap();
    let proposal = snapshot["nodes"].as_array().unwrap().iter().find(|n| n["status"] == "proposal").unwrap()["entryId"].as_str().unwrap().to_owned();
    let arguments = json!({"action": "approve", "ids": [proposal.clone()]}).to_string();
    let call = json!({"id": "call_m", "type": "function", "function": {"name": "WikiManage", "arguments": arguments}});
    let (tx, mut requests, running, mut output) = start_with_output(Some(f.wiki.clone()), vec![call]).await;
    send(&tx, 1, "approve the pagination proposal please").await;
    let _ = next_turn(&mut requests, "approve the pagination proposal please").await;
    // 权限确认挂起：既没有下一次模型请求，条目也没有被批准。
    let waited = tokio::time::timeout(Duration::from_millis(1500), async {
        loop {
            let (messages, _) = next(&mut requests).await;
            if messages.iter().any(|m| m["role"] == "tool") {
                return messages;
            }
        }
    })
    .await;
    assert!(waited.is_err(), "WikiManage must not execute before the user approves");
    let (_, entry) = f.wiki.locate(&proposal).await.unwrap();
    assert_eq!(entry.meta.status.as_str(), "proposal");
    // 输出里出现了权限交互（alwaysAsk），而不是工具结果。
    let mut saw_permission = false;
    while let Ok(Some(batch)) = tokio::time::timeout(Duration::from_millis(200), output.recv()).await {
        saw_permission |= serde_json::to_string(&batch).unwrap().contains("permission");
    }
    assert!(saw_permission, "a permission interaction is published");
    stop(tx, running).await;
}
