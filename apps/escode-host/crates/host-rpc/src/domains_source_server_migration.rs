//! `source-server-management` 迁移方法（S4-f，spec §11.6）：报告读取/导出与重试/放弃。

use std::sync::Arc;

use escode_host_services::source_server::SourceServerRegistry;
use escode_host_services::source_server_connections::SourceServerConnections;
use serde_json::Value;

use crate::channel::{MethodHandler, handler_rpc};
use crate::session::RpcError;
use crate::wire::Wire;

use super::string_arg;

pub(super) fn migration_methods(
    registry: Arc<SourceServerRegistry>,
    connections: Arc<SourceServerConnections>,
) -> Vec<(&'static str, MethodHandler)> {
    vec![
        (
            "getMigrationReport",
            migration_report_method(Arc::clone(&connections)),
        ),
        (
            "exportMigrationReport",
            export_migration_report_method(Arc::clone(&connections)),
        ),
        (
            "abandonWorkspaceMigration",
            path_action_method(
                Arc::clone(&registry),
                Arc::clone(&connections),
                "abandonWorkspaceMigration",
                true,
            ),
        ),
        (
            "retryWorkspaceMigration",
            path_action_method(registry, connections, "retryWorkspaceMigration", false),
        ),
    ]
}

fn migration_report_method(connections: Arc<SourceServerConnections>) -> MethodHandler {
    handler_rpc(move |arg: Wire| {
        let connections = Arc::clone(&connections);
        Box::pin(async move {
            let source_server_id = string_arg(&arg, 0, "getMigrationReport")?;
            let report = connections
                .migration_report(&source_server_id)
                .await
                .map_err(|error| RpcError::new("Error", format!("{error:#}")))?;
            Ok(Wire::Object(report))
        })
    })
}

fn export_migration_report_method(connections: Arc<SourceServerConnections>) -> MethodHandler {
    handler_rpc(move |arg: Wire| {
        let connections = Arc::clone(&connections);
        Box::pin(async move {
            let source_server_id = string_arg(&arg, 0, "exportMigrationReport")?;
            let report = connections
                .migration_report(&source_server_id)
                .await
                .map_err(|error| RpcError::new("Error", format!("{error:#}")))?;
            let text = export_migration_report_text(&report)
                .map_err(|error| RpcError::new("Error", format!("{error:#}")))?;
            Ok(Wire::Str(text))
        })
    })
}

fn path_action_method(
    registry: Arc<SourceServerRegistry>,
    connections: Arc<SourceServerConnections>,
    method: &'static str,
    abandon: bool,
) -> MethodHandler {
    handler_rpc(move |arg: Wire| {
        let registry = Arc::clone(&registry);
        let connections = Arc::clone(&connections);
        Box::pin(async move {
            let source_server_id = string_arg(&arg, 0, method)?;
            let workspace_path = string_arg(&arg, 1, method)?;
            let migration_id = string_arg(&arg, 2, method)?;
            let result = if abandon {
                connections
                    .abandon_workspace_migration(&source_server_id, &workspace_path, &migration_id)
                    .await
            } else {
                connections
                    .retry_workspace_migration(&source_server_id, &workspace_path, &migration_id)
                    .await
            };
            // finally 语义：远端成功或失败都让统计失效并通知订阅端重读。
            connections.invalidate_migration_counts(&source_server_id);
            registry.publish_directory_change();
            result
                .map(Wire::Object)
                .map_err(|error| RpcError::new("Error", format!("{error:#}")))
        })
    })
}

/// 导出格式与 TS `JSON.stringify(report, null, 2) + "\n"` 对齐。
pub(super) fn export_migration_report_text(report: &Value) -> serde_json::Result<String> {
    let mut text = serde_json::to_string_pretty(report)?;
    text.push('\n');
    Ok(text)
}

/// 严格解析 `{ refreshMigrationCounts?: boolean }`；未知字段/错误类型直接拒绝。
pub(super) fn parse_snapshot_request(arg: &Wire) -> Result<bool, RpcError> {
    let request = match arg {
        Wire::Undefined => None,
        Wire::Array(items) if items.is_empty() => None,
        Wire::Array(items) if items.len() == 1 => Some(&items[0]),
        Wire::Object(_) => Some(arg),
        _ => return Err(invalid_options()),
    };
    let Some(request) = request else {
        return Ok(false);
    };
    let Wire::Object(object) = request else {
        return Err(invalid_options());
    };
    let Some(object) = object.as_object() else {
        return Err(invalid_options());
    };
    for key in object.keys() {
        if key != "refreshMigrationCounts" {
            return Err(RpcError::new(
                "InvalidRequest",
                format!("unsupported getSnapshot option: {key}"),
            ));
        }
    }
    match object.get("refreshMigrationCounts") {
        None => Ok(false),
        Some(value) => value.as_bool().ok_or_else(|| {
            RpcError::new("InvalidRequest", "refreshMigrationCounts must be a boolean")
        }),
    }
}

fn invalid_options() -> RpcError {
    RpcError::new("InvalidRequest", "getSnapshot requires an options object")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn snapshot_options_are_strict() {
        assert!(!parse_snapshot_request(&Wire::Undefined).unwrap());
        assert!(!parse_snapshot_request(&Wire::Array(Vec::new())).unwrap());
        assert!(
            parse_snapshot_request(&Wire::Array(vec![Wire::Object(json!({
                "refreshMigrationCounts": true
            }))]))
            .unwrap()
        );
        assert!(
            parse_snapshot_request(&Wire::Array(vec![Wire::Object(json!({
                "unknown": true
            }))]))
            .is_err()
        );
        assert!(
            parse_snapshot_request(&Wire::Array(vec![Wire::Object(json!({
                "refreshMigrationCounts": "yes"
            }))]))
            .is_err()
        );
    }

    #[test]
    fn export_matches_typescript_pretty_json_with_newline() {
        let report = json!({ "rows": [{ "migrationId": "m1", "status": "completed" }] });
        let text = export_migration_report_text(&report).unwrap();
        assert!(text.ends_with('\n'));
        assert!(text.contains("\n  \"rows\""));
        assert_eq!(serde_json::from_str::<Value>(&text).unwrap(), report);
    }
}
