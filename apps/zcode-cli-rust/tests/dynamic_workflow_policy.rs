//! 动态工作流灰度门的协议面（docs/specs/rust-dynamic-workflow.md 第 1 期）：
//! `workspace/updateDynamicWorkflowPolicy` 的 strict 参数、回显与工作区校验。
//! 读法（fail-closed、创建参数优先、翻转不回收已固化会话）由 `domain::dynamic_workflow` 单测覆盖。
use anyhow::Result;
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc};
use tokio_util::sync::CancellationToken;
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
    async fn commit(
        &self,
        _: &str,
        _: Option<&mut Session>,
        _: Option<(String, Value)>,
    ) -> Result<()> {
        Ok(())
    }
}
struct Model;
#[async_trait]
impl ModelPort for Model {
    async fn complete(
        &self,
        _: Vec<Value>,
        _: &[Value],
        _: &EventSink,
        _: &CancellationToken,
    ) -> std::result::Result<ModelOutput, ModelFailure> {
        panic!("no model call expected")
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
struct Clock;
impl RuntimeClock for Clock {
    fn id(&self) -> String {
        "id".into()
    }
    fn now(&self) -> u64 {
        1000
    }
}

async fn engine() -> Engine {
    Engine::new(
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
            model: Some(Arc::new(Model)),
            tools: Arc::new(Tools),
            clock: Arc::new(Clock),
        },
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn policy_method_echoes_the_workspace_and_rejects_bad_params() {
    let mut engine = engine().await;
    let workspace = json!({"workspacePath":"workspace","workspaceIdentity":"workspace"});
    let update = |enabled: Value, workspace: Value| json!({"workspace":workspace,"enabled":enabled});
    // 回显与 TS `updateDynamicWorkflowPolicy` 同形：`{workspace, enabled}`。
    assert_eq!(
        engine
            .query_method(
                "workspace/updateDynamicWorkflowPolicy",
                &update(json!(true), workspace.clone())
            )
            .unwrap(),
        update(json!(true), workspace.clone())
    );
    assert_eq!(
        engine
            .query_method(
                "workspace/updateDynamicWorkflowPolicy",
                &update(json!(false), workspace.clone())
            )
            .unwrap(),
        update(json!(false), workspace.clone())
    );
    // strict `{workspace, enabled}`：多余键、非布尔 enabled、缺 workspace、工作区不一致都拒绝。
    for params in [
        json!({"workspace":workspace,"enabled":true,"extra":1}),
        update(json!("yes"), workspace.clone()),
        json!({"enabled":true}),
        json!({"workspace":{"workspacePath":"other"},"enabled":true}),
    ] {
        assert!(
            engine
                .query_method("workspace/updateDynamicWorkflowPolicy", &params)
                .is_err(),
            "{params}"
        );
    }
}
