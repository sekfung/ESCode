//! `workflows/*` GUI 中枢方法（TS saved-workflows hub）：list / get / updateMeta / delete / move。
#[allow(unused_imports)]
use super::saved_workflows::*;

use std::path::Path;
use escode_cli_domain::{json_order::Json, saved_workflow as codec};

/// 失败结果（TS `toFailure` / `escodeSavedWorkflowFailureSchema`）：`parse_error` 与 `read_error`
/// 只带 detail——协议面**不**回文件路径，避免把中枢变成路径探测口。
pub(super) fn failure(reason: &str, detail: Option<String>) -> Json {
    let mut value = Json::object();
    value.set("ok", Json::Bool(false));
    value.set("reason", Json::str(reason));
    if let Some(detail) = detail {
        value.set("detail", Json::str(detail));
    }
    value
}

pub(super) fn resolve_failure(resolved: Resolve) -> Json {
    match resolved {
        Resolve::InvalidName { detail } => failure("invalid_name", Some(detail)),
        Resolve::NotFound => failure("not_found", None),
        Resolve::ParseError { detail, .. } => failure("parse_error", Some(detail)),
        Resolve::ReadError { detail, .. } => failure("read_error", Some(detail)),
        Resolve::Found(_) => unreachable!("调用方已处理成功分支"),
    }
}

/// `workflows/list`（TS `listSavedWorkflowsOp`）：**定向** scope 的枚举 + 扫过的目录（即使不存在也回，
/// GUI 的文件监听靠它 watch）。与工具输出不同，`invalid` 在这里恒在场。
pub fn hub_list(cwd: &Path, home: &Path, scope: Scope) -> Json {
    let listed = list(cwd, home, Some(scope));
    let mut value = Json::object();
    value.set(
        "workflows",
        Json::Array(listed.entries.iter().map(entry_json).collect()),
    );
    value.set(
        "invalid",
        Json::Array(
            listed
                .invalid
                .iter()
                .map(|invalid| {
                    let mut entry = Json::object();
                    entry.set(
                        "path",
                        Json::str(invalid.path.to_string_lossy().into_owned()),
                    );
                    entry.set("reason", Json::str(&invalid.reason));
                    entry
                })
                .collect(),
        ),
    );
    value.set(
        "dir",
        Json::str(root(cwd, scope, home).dir.to_string_lossy().into_owned()),
    );
    value
}

/// `workflows/get`（TS `getSavedWorkflowOp`）：定向 scope 解析；成功带 meta 与脚本正文。
pub fn hub_get(cwd: &Path, home: &Path, name: &str, scope: Scope) -> Json {
    match resolve(cwd, home, name, Some(scope)) {
        Resolve::Found(resolved) => {
            let mut value = Json::object();
            value.set("ok", Json::Bool(true));
            value.set("name", Json::str(resolved.name));
            value.set(
                "path",
                Json::str(resolved.path.to_string_lossy().into_owned()),
            );
            value.set("scope", Json::str(resolved.scope.as_str()));
            value.set("meta", resolved.meta);
            value.set("script", Json::str(resolved.script));
            value
        }
        failure => resolve_failure(failure),
    }
}

/// `workflows/updateMeta`（TS `updateSavedWorkflowMetaOp`）：读回当前脚本正文，整文件覆写为
/// `serialize(newMeta, script)`。读-改-写在同一次调用内完成（文件小、单机、用户自己在改）。
pub fn hub_update_meta(
    cwd: &Path,
    home: &Path,
    name: &str,
    meta: &Json,
    scope: Scope,
) -> Result<Json, Vec<String>> {
    let issues = codec::validate(meta);
    if !issues.is_empty() {
        return Err(issues);
    }
    let resolved = match resolve(cwd, home, name, Some(scope)) {
        Resolve::Found(resolved) => resolved,
        failure => return Ok(resolve_failure(failure)),
    };
    let text = codec::serialize(meta, &resolved.script);
    if let Err(error) = std::fs::write(&resolved.path, text) {
        return Ok(failure("read_error", Some(describe(&error))));
    }
    let mut value = Json::object();
    value.set("ok", Json::Bool(true));
    value.set(
        "path",
        Json::str(resolved.path.to_string_lossy().into_owned()),
    );
    Ok(value)
}

/// `workflows/delete`（TS `deleteSavedWorkflowOp`）：只按名字删，名字先验后拼路径（路径穿越的防线）。
pub fn hub_delete(cwd: &Path, home: &Path, name: &str, scope: Scope) -> Json {
    if !is_valid_name(name) {
        // TS 这一步**不带** detail：删除的非法名字不给任何额外信息。
        return failure("invalid_name", None);
    }
    let path = saved_path(&root(cwd, scope, home), name);
    match std::fs::remove_file(&path) {
        Ok(()) => {
            let mut value = Json::object();
            value.set("ok", Json::Bool(true));
            value.set("path", Json::str(path.to_string_lossy().into_owned()));
            value
        }
        Err(error) if is_not_found(&error) => failure("not_found", None),
        Err(error) => failure("read_error", Some(describe(&error))),
    }
}

/// `workflows/move`（TS `moveSavedWorkflowOp`）：全局档 → 本项目，逐字节搬、不覆盖。
pub fn hub_move(cwd: &Path, home: &Path, name: &str) -> Json {
    match move_to_project(cwd, home, name) {
        Move::Ok { from, to } => {
            let mut value = Json::object();
            value.set("ok", Json::Bool(true));
            value.set("from", Json::str(from.to_string_lossy().into_owned()));
            value.set("to", Json::str(to.to_string_lossy().into_owned()));
            value
        }
        Move::InvalidName { detail } => failure("invalid_name", Some(detail)),
        Move::NotFound => failure("not_found", None),
        Move::TargetExists { path } => {
            let mut value = failure("target_exists", None);
            value.set("path", Json::str(path.to_string_lossy().into_owned()));
            value
        }
        Move::ReadError { path, detail } => {
            let mut value = failure("read_error", Some(detail));
            value.set("path", Json::str(path.to_string_lossy().into_owned()));
            value
        }
        Move::WriteError { path, detail } => {
            let mut value = failure("write_error", Some(detail));
            value.set("path", Json::str(path.to_string_lossy().into_owned()));
            value
        }
    }
}

/// `workflows/*` 的协议分派：workspace 级、无会话，每次调用现扫目录（挂载时快照会漏掉用户手改或模型刚落盘的文件）。
pub(crate) fn op(op: &str, params: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
    use super::saved_workflows as store;
    use anyhow::{Context, bail};
    use serde_json::Value;
    let cwd = std::path::PathBuf::from(
        params["workspace"]["workspacePath"]
            .as_str()
            .context("Workspace path required")?,
    );
    let home = std::path::PathBuf::from(escode_cli_host::credential_cipher::node_homedir());
    // 缺省即 `project`：不给 scope 的旧 GUI 与项目档调用逐字走本项目根。
    let scope = match params.get("scope").and_then(Value::as_str) {
        Some(scope) => store::Scope::parse(scope).context("Invalid scope")?,
        None => store::Scope::Project,
    };
    let name = || params["name"].as_str().context("Invalid name");
    let result = match op {
        "list" => store::hub_list(&cwd, &home, scope),
        "get" => store::hub_get(&cwd, &home, name()?, scope),
        "updateMeta" => {
            let meta = escode_cli_domain::json_order::Json::parse(&params["meta"].to_string())
                .context("Invalid meta")?;
            store::hub_update_meta(&cwd, &home, name()?, &meta, scope)
                .map_err(|issues| anyhow::anyhow!(issues.join("; ")))?
        }
        "delete" => store::hub_delete(&cwd, &home, name()?, scope),
        "move" => store::hub_move(&cwd, &home, name()?),
        other => bail!("Unsupported saved workflow operation: {other}"),
    };
    Ok(store::to_value(&result))
}
