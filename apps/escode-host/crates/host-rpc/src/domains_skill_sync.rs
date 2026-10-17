//! `skill-sync` channel（TS `ServiceChannels.SkillSync` = `"skill-sync"`；docs/specs/rust-u2-sync.md §1）。
//!
//! Host 级服务：用户级 skills 的候选、远端状态、归档导出/导入与写权限探测。

use std::path::Path;
use std::sync::Arc;

use escode_host_services::skill_sync::{SkillSyncError, SkillSyncService};
use escode_host_services::skills::Compare;
use serde_json::Value;

use crate::channel::Channel;
use crate::domains_task_support::first_object;
use crate::session::{RpcError, handler_rpc};
use crate::wire::Wire;

/// TS `SkillSyncSizeLimitError`：稳定 code + 结构化 data；其余是普通 Error。
fn skill_sync_error(error: SkillSyncError) -> RpcError {
    let name = if error.size_limit.is_some() {
        "SkillSyncSizeLimitError"
    } else {
        "Error"
    };
    let (code, data) = (error.code(), error.data());
    let mut rpc = RpcError::new(name, error.message);
    if let Some(code) = code {
        rpc = rpc.with("code", Value::from(code));
    }
    if let Some(data) = data {
        rpc = rpc.with("data", data);
    }
    rpc
}

/// JS 真值判断（TS 的 `if (params.overwrite)`）。
fn js_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_some_and(|value| value != 0.0),
        Value::String(text) => !text.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

pub(crate) fn skill_sync_channel(home: &Path) -> Channel {
    let compare: Compare = Arc::new(crate::collation::compare_names);
    let service = Arc::new(SkillSyncService::new(home.to_path_buf(), compare));
    let list_candidates = {
        let service = Arc::clone(&service);
        handler_rpc(move |_arg: Wire| {
            let service = Arc::clone(&service);
            Box::pin(async move {
                service
                    .list_local_user_skill_candidates()
                    .await
                    .map(Wire::Object)
                    .map_err(skill_sync_error)
            })
        })
    };
    let list_statuses = {
        let service = Arc::clone(&service);
        handler_rpc(move |arg: Wire| {
            let (service, params) = (Arc::clone(&service), first_object(arg));
            Box::pin(async move {
                service
                    .list_remote_user_skill_statuses(&params)
                    .await
                    .map(Wire::Object)
                    .map_err(skill_sync_error)
            })
        })
    };
    let export = {
        let service = Arc::clone(&service);
        handler_rpc(move |arg: Wire| {
            let (service, params) = (Arc::clone(&service), first_object(arg));
            Box::pin(async move {
                let result = service
                    .export_skills_archive(&params)
                    .await
                    .map_err(skill_sync_error)?;
                Ok(Wire::Object(serde_json::json!({
                    "archive": crate::wire_bytes::encode(&result.archive),
                    "archiveBytes": result.archive_bytes,
                    "skills": result.skills,
                })))
            })
        })
    };
    let check_write_access = {
        let service = Arc::clone(&service);
        handler_rpc(move |_arg: Wire| {
            let service = Arc::clone(&service);
            Box::pin(async move {
                service
                    .check_remote_user_skill_write_access()
                    .await
                    .map(Wire::Object)
                    .map_err(skill_sync_error)
            })
        })
    };
    let import = {
        let service = Arc::clone(&service);
        handler_rpc(move |arg: Wire| {
            let (service, params) = (Arc::clone(&service), first_object(arg));
            Box::pin(async move {
                let archive = crate::wire_bytes::decode(&params["archive"])
                    .map_err(|message| RpcError::new("Error", message))?;
                let overwrite = js_truthy(params.get("overwrite").unwrap_or(&Value::Null));
                service
                    .import_skills_archive(archive, overwrite)
                    .await
                    .map(Wire::Object)
                    .map_err(skill_sync_error)
            })
        })
    };
    Channel::new(
        "skill-sync",
        vec![
            ("listLocalUserSkillCandidates", list_candidates),
            ("listRemoteUserSkillStatuses", list_statuses),
            ("exportSkillsArchive", export),
            ("checkRemoteUserSkillWriteAccess", check_write_access),
            ("importSkillsArchive", import),
        ],
    )
}
