//! Desktop 知识图谱协议（apps/escode-cli-rust/docs/llm-wiki/01-spec.md §11）：`v4/wiki/status|graph|entry|manage`。
//! 导入/同步可能耗时，因此与插件作业相同，在后台任务里执行并经 `AuxiliaryReply` 回复，不阻塞 Engine actor。
//! 端口缺席（未组合 Wiki）时：status 返回 `supported=false`，其余明确报错。
use super::auxiliary::Auxiliary;
use super::engine::Engine;
use crate::contract::{Event, EventSink};
use crate::domain::protocol::Request;
use anyhow::{Result, ensure};
use serde_json::json;
use tokio_util::sync::CancellationToken;

pub(super) const WIKI_METHODS: &[&str] = &[
    "v4/wiki/status",
    "v4/wiki/graph",
    "v4/wiki/entry",
    "v4/wiki/manage",
];
const MAX_PARALLEL: usize = 16;

impl Engine {
    pub(super) fn start_wiki_request(&mut self, request: &Request) -> Result<()> {
        ensure!(
            self.auxiliary.len() < MAX_PARALLEL,
            "Too many workspace requests"
        );
        let id = format!("wiki-op:{}", self.clock.id());
        self.auxiliary.insert(
            id.clone(),
            Auxiliary {
                request: request.id.clone(),
                cancel: CancellationToken::new(),
                operation: None,
                session: None,
            },
        );
        let sink = EventSink {
            session_id: id.clone(),
            run_id: id,
            tx: self.events.clone(),
        };
        let wiki = self.wiki.clone();
        let (method, params) = (request.method.clone(), request.params.clone());
        self.spawner.spawn("wiki-request", async move {
            let result = match wiki {
                Some(port) => port
                    .request(&method, &params)
                    .await
                    .map_err(|e| format!("{e:#}")),
                None if method == "v4/wiki/status" => {
                    Ok(json!({"supported": false, "enabled": false}))
                }
                None => Err("wiki_unavailable: this runtime has no LLM Wiki configured".to_owned()),
            };
            let _ = sink.send(Event::AuxiliaryReply { result }).await;
        });
        Ok(())
    }
}
