//! `subagents/*` 查询（会话的子代理任务清单）。
use super::Engine;
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};

impl Engine {
    pub(super) async fn subagents_query(&mut self, p: &Value) -> Result<Value> {
        let id = p["sessionId"].as_str().context("Session required")?;
        self.ensure_session(id).await?;
        let s = &self.sessions[id];
        let offset = p["endedCursor"]
            .as_str()
            .map(str::parse::<usize>)
            .transpose()?
            .unwrap_or(0);
        let limit = p["endedLimit"].as_u64().unwrap_or(20);
        ensure!((1..=100).contains(&limit), "Invalid ended limit");
        let mut ended = s
            .children
            .values()
            .filter(|t| !t.running())
            .collect::<Vec<_>>();
        ended.sort_by_key(|t| std::cmp::Reverse(t.ended_at));
        ensure!(offset <= ended.len(), "Invalid ended cursor");
        let items = ended
            .iter()
            .skip(offset)
            .take(limit as usize)
            .map(|t| t.summary())
            .collect::<Vec<_>>();
        let mut end = json!({"total":ended.len(),"items":items});
        if offset + items.len() < ended.len() {
            end["nextCursor"] = (offset + items.len()).to_string().into();
        }
        Ok(
            json!({"revision":s.revision,"childSessionIds":s.children.values().map(|t|&t.child_id).collect::<Vec<_>>(),"running":s.children.values().filter(|t|t.running()).map(|t|t.summary()).collect::<Vec<_>>(),"ended":end}),
        )
    }
}
