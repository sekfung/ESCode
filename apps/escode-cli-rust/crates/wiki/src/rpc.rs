//! 管理动作与 Desktop 协议（spec §11–§12）：`manage` 被 WikiManage 工具、`v4/wiki/manage` 与 CLI 共用；
//! `v4/wiki/status|graph|entry` 只读。所有入口都经同一个 WikiService，作用域固定为 global + 当前项目。
use super::service::WikiService;
use anyhow::{Result, bail};
use escode_cli_domain::wiki::entry::{Kind, Scope, Status};
use escode_cli_domain::wiki::graph::{self, Filter};
use escode_cli_domain::wiki::source::SourceRecord;
use escode_cli_domain::wiki::tools::ManageInput;
use serde_json::{Value, json};
use std::path::PathBuf;

fn scope_param(value: Option<&str>) -> Result<Scope> {
    match value {
        None | Some("project") => Ok(Scope::Project),
        Some("global") => Ok(Scope::Global),
        Some(other) => bail!("invalid scope '{other}'"),
    }
}

fn items<T: serde::Serialize>(values: &[T]) -> Value {
    serde_json::to_value(values).unwrap_or(Value::Null)
}

impl WikiService {
    /// `by`：发起方（cli / desktop / agent:<session> (user-approved)），写入审核与审计记录。
    pub async fn manage(&self, input: &ManageInput, by: &str) -> Result<Value> {
        let ids = input.ids.clone().unwrap_or_default();
        let result = match input.action.as_str() {
            "status" => {
                let status = self.status().await?;
                json!({"ok": true, "summary": "wiki status", "items": [status]})
            }
            "lint" => {
                let issues = self.lint().await?;
                let errors = issues.iter().filter(|i| i.severity == "error").count();
                json!({"ok": errors == 0, "summary": format!("{} lint issue(s), {errors} error(s)", issues.len()), "items": items(&issues)})
            }
            "validate" => {
                let drift = self.validate_sources().await?;
                json!({"ok": drift.is_empty(), "summary": format!("{} source(s) changed, missing or failed; run sync to update", drift.len()), "items": drift})
            }
            "conflicts" => {
                let conflicts = self.conflicts().await?;
                json!({"ok": conflicts.is_empty(), "summary": format!("{} conflict(s)", conflicts.len()), "items": items(&conflicts)})
            }
            "ingest" => {
                let paths: Vec<PathBuf> = input
                    .paths
                    .clone()
                    .unwrap_or_default()
                    .into_iter()
                    .map(PathBuf::from)
                    .collect();
                if paths.is_empty() {
                    bail!("ingest needs paths");
                }
                let report = self
                    .ingest(
                        &paths,
                        scope_param(input.scope.as_deref())?,
                        input.approve_imported == Some(true),
                    )
                    .await?;
                json!({"ok": report.ok(), "summary": report.summary(), "items": items(&report.items)})
            }
            "sync" => {
                let mut all = vec![];
                let mut summaries = vec![];
                let scopes = match input.scope.as_deref() {
                    Some(s) => vec![scope_param(Some(s))?],
                    None => vec![Scope::Project, Scope::Global],
                };
                for scope in scopes {
                    let report = self.sync(scope).await?;
                    summaries.push(report.summary());
                    all.extend(report.items);
                }
                json!({"ok": all.iter().all(|i| i.status != "failed"), "summary": summaries.join("; "), "items": items(&all)})
            }
            "approve" | "reject" | "deprecate" => {
                let results = self
                    .review(
                        &input.action,
                        &ids,
                        input.reason.as_deref(),
                        input.superseded_by.as_deref(),
                        by,
                    )
                    .await?;
                let ok = results.iter().all(|(_, r)| !r.starts_with("error"));
                json!({"ok": ok, "summary": format!("{} {} request(s)", results.len(), input.action), "items": results.iter().map(|(id, r)| json!({"ref": id, "status": r})).collect::<Vec<_>>()})
            }
            "promote" => {
                let mut out = vec![];
                for id in &ids {
                    out.push(match self.promote(id, by).await {
                        Ok(new_id) => json!({"ref": id, "status": "promoted", "message": format!("global proposal {new_id}")}),
                        Err(e) => json!({"ref": id, "status": "error", "message": format!("{e:#}")}),
                    });
                }
                json!({"ok": out.iter().all(|o| o["status"] == "promoted"), "summary": format!("{} promote request(s)", out.len()), "items": out})
            }
            "rebuild_index" => {
                let rebuilt = self.rebuild_indexes().await?;
                json!({"ok": true, "summary": "indexes rebuilt from entry files", "items": rebuilt.iter().map(|(s, v, n)| json!({"ref": s, "status": "rebuilt", "message": format!("version {v}, {n} entries")})).collect::<Vec<_>>()})
            }
            other => bail!("unknown wiki action '{other}'"),
        };
        if !matches!(
            input.action.as_str(),
            "status" | "lint" | "validate" | "conflicts"
        ) {
            // 生命周期事件：只记动作、发起方、结果与摘要（摘要不含文档正文）。
            escode_cli_host::file_log::info(
                "wiki.manage",
                "wiki",
                result["summary"].as_str().unwrap_or_default(),
                json!({"action": input.action, "by": by, "ok": result["ok"], "ids": ids}),
            );
        }
        Ok(
            json!({"action": input.action, "ok": result["ok"], "summary": result["summary"], "items": result["items"]}),
        )
    }

    pub async fn entry_detail(&self, id: &str) -> Result<Value> {
        let snapshots = self.open_snapshots().await?;
        let Some((scope, entry)) = Self::scope_of(&snapshots, id) else {
            return Ok(json!({"found": false}));
        };
        let store = if scope == Scope::Project {
            &snapshots[0].store
        } else {
            &snapshots[1].store
        };
        let records: Vec<SourceRecord> = store.sources().await?;
        let sources: Vec<Value> = entry.meta.sources.iter().map(|s| {
            let record = records.iter().find(|r| r.document_id == s.document_id);
            let state = match record {
                Some(r) if r.content_hash != s.content_hash && r.status.as_str() == "current" => "changed".to_owned(),
                Some(r) => r.status.as_str().to_owned(),
                None => "unknown".to_owned(),
            };
            json!({"document_id": s.document_id, "source_path": record.map_or(s.source_path.clone(), |r| r.source_path.clone()), "format": s.format, "locator": s.locator.value, "excerpt": s.excerpt, "content_hash": s.content_hash, "source_status": state})
        }).collect();
        let conflicts: Vec<Value> = self
            .conflicts()
            .await?
            .into_iter()
            .filter(|c| c.parties.iter().any(|p| p.id == id))
            .map(|c| serde_json::to_value(c).unwrap_or(Value::Null))
            .collect();
        Ok(json!({
            "found": true,
            "entry": {
                "id": entry.meta.id, "title": entry.meta.title, "scope": entry.meta.scope.as_str(), "kind": entry.meta.kind.as_str(),
                "status": entry.meta.status.as_str(), "authority": entry.meta.authority.as_str(), "validity": entry.meta.validity.state.as_str(),
                "validity_reason": entry.meta.validity.reason, "origin": entry.meta.origin.as_str(), "tags": entry.meta.tags,
                "related": entry.meta.related, "supersedes": entry.meta.supersedes, "superseded_by": entry.meta.superseded_by,
                "applies_to": entry.meta.applies_to.paths, "updated_at": entry.meta.updated_at, "created_at": entry.meta.created_at,
                "body": entry.body, "sources": sources,
            },
            "conflicts": conflicts,
            "review": serde_json::to_value(&entry.meta.review).unwrap_or(Value::Null),
        }))
    }

    pub async fn graph(&self, params: &Value) -> Result<Value> {
        let settings = self.settings().await;
        let project = self.project().await.clone();
        if !settings.config.enabled {
            return Ok(
                json!({"enabled": false, "project": {"id": project.id, "name": project.name}, "nodes": [], "edges": [], "truncated": false, "counts": {}}),
            );
        }
        let parse_list = |key: &str| {
            params[key].as_array().map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
        };
        let filter = Filter {
            scopes: parse_list("scopes")
                .map(|l| l.iter().filter_map(|s| Scope::parse(s)).collect()),
            statuses: parse_list("statuses")
                .map(|l| l.iter().filter_map(|s| Status::parse(s)).collect()),
            kinds: parse_list("kinds").map(|l| l.iter().filter_map(|s| Kind::parse(s)).collect()),
            include_sources: params["includeSources"].as_bool().unwrap_or(true),
            limit: params["limit"].as_u64().map_or(500, |l| l as usize),
        };
        let snapshots = self.open_snapshots().await?;
        let entries: Vec<_> = snapshots
            .iter()
            .flat_map(|s| s.entries.values().cloned())
            .collect();
        let mut sources = vec![];
        for s in &snapshots {
            sources.extend(s.store.sources().await?);
        }
        let conflicts = self.conflicts().await?;
        let built = graph::build(&entries, &sources, &conflicts, &filter);
        let mut value = serde_json::to_value(built)?;
        value["enabled"] = true.into();
        value["project"] =
            json!({"id": project.id, "name": project.name, "identityKind": project.kind});
        Ok(value)
    }

    pub async fn request(&self, method: &str, params: &Value) -> Result<Value> {
        match method {
            "v4/wiki/status" => {
                let mut status = self.status().await?;
                status["supported"] = true.into();
                Ok(status)
            }
            "v4/wiki/graph" => self.graph(params).await,
            "v4/wiki/entry" => {
                let id = params["id"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| anyhow::anyhow!("id is required"))?;
                self.entry_detail(id).await
            }
            "v4/wiki/manage" => {
                let input = ManageInput {
                    action: params["action"].as_str().unwrap_or_default().to_owned(),
                    scope: params["scope"].as_str().map(str::to_owned),
                    paths: params["paths"].as_array().map(|a| {
                        a.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect()
                    }),
                    ids: params["ids"].as_array().map(|a| {
                        a.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect()
                    }),
                    reason: params["reason"].as_str().map(str::to_owned),
                    superseded_by: params["supersededBy"].as_str().map(str::to_owned),
                    approve_imported: params["approveImported"].as_bool(),
                };
                self.manage(&input, "desktop").await
            }
            other => bail!("unknown wiki method {other}"),
        }
    }
}
