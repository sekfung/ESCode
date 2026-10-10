//! 本地 TTFT 观测的帧附着（TS v4-gateway reserveTopicFrame）：首输出时刻只经在线增量帧送达渲染端。
use super::Engine;
use serde_json::Value;

impl Engine {
    /// 增量帧的 TTFT 观测（TS v4-gateway 的帧附着）：按本批增量触及的回合找其发起命令的记录，找不到时取本会话最近一条
    /// 已起跑的记录。首条进 `ttft`，其余（至多 16 条）进 `ttftRelated`。
    pub(super) fn local_ttft_frame(
        &self,
        session: &str,
        payload: &Value,
    ) -> Option<(Value, Vec<Value>)> {
        let all: Vec<&Value> = self
            .local_ttft
            .completed
            .iter()
            .chain(&self.local_ttft.records)
            .map(|(_, r)| r)
            .filter(|r| r["sessionId"] == session)
            .collect();
        if !all.iter().any(|r| r.get("turnId").is_some()) {
            return None;
        }
        let rows = &self.sessions.get(session)?.rows;
        let mut turns: Vec<&str> = vec![];
        for delta in payload["deltas"].as_array().into_iter().flatten() {
            let turn = match delta["op"].as_str() {
                Some("row.appended" | "row.upserted") => delta["row"]["turnId"].as_str(),
                Some("row.delta") => rows
                    .iter()
                    .find(|r| r["rowId"] == delta["rowId"])
                    .and_then(|r| r["turnId"].as_str()),
                _ => None,
            };
            if let Some(turn) = turn.filter(|t| !turns.contains(t)) {
                turns.push(turn);
            }
        }
        let header_of = |command: &str| {
            rows.iter()
                .find(|r| r["kind"] == "turnHeader" && r["sourceCommandId"] == command)
        };
        let mut related: Vec<&Value> = rows
            .iter()
            .filter(|r| {
                r["kind"] == "turnHeader"
                    && turns.contains(&r["turnId"].as_str().unwrap_or_default())
            })
            .filter_map(|h| h["sourceCommandId"].as_str())
            .filter_map(|command| {
                all.iter()
                    .rev()
                    .find(|r| r["commandId"] == command)
                    .copied()
            })
            .collect();
        if related.is_empty() {
            related = all
                .iter()
                .rev()
                .find(|r| r.get("turnId").is_some())
                .copied()
                .into_iter()
                .collect();
        }
        let mut observations: Vec<Value> = vec![];
        for record in related {
            if observations
                .iter()
                .any(|o| o["observationId"] == record["observationId"])
            {
                continue;
            }
            let mut observation = record.clone();
            // TS host.cliVersion：本 runtime 的版本标识（观测归因）。
            observation["cliVersion"] = concat!("rust-", env!("CARGO_PKG_VERSION")).into();
            if let Some(header) = record["commandId"].as_str().and_then(header_of) {
                observation["productTurnId"] = header["turnId"].clone();
            }
            observations.push(observation);
        }
        let mut observations = observations.into_iter();
        let first = observations.next()?;
        Some((first, observations.take(16).collect()))
    }
}
