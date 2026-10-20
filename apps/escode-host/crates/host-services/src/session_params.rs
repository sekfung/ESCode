//! `escode-session.createSession` / `resumeSession` 的参数整形（S2/S3 写入口的前置件）。
//!
//! 上游真相源：
//! - `packages/services/src/escode-agent/escodeAgentService.ts:692` 的 `buildSessionCreateParams`
//!   与 `:761` 的 `buildSessionResumeParams`（键白名单 + 「可选字段缺省即省略」规则）；
//! - `packages/services/src/session/mcpWorkspaceScope.ts` 的 `appendWorkspaceToFilesystemMcpServers`
//!   （filesystem MCP 追加当前 workspace，避免 agent 写本机文件被拒）。
//!
//! 返回的是**服务级参数**（`forward::runtime_target` 会在其上补 `workspace` 引用并剥掉路由键），
//! 与 TS 交给 agent 层的形状逐键一致（唯一差别是 TS 的 `workspace` 引用由 Host 侧转发层构造）。

use serde_json::{Map, Value};

/// Host 侧判定的工具面灰度（TS `resolveDynamicWorkflowSessionFlags`）。
///
/// **不从 Renderer 入参读取**（评审 A12）：TS 用 `{...params, ...flags}` 以 Host 判定值覆盖入参。
/// 判定依赖服务端 client-config（对应域未迁移），在那之前一律关闭（缺省即 fail-closed，
/// 见 docs/specs/rust-u2-agent-v4.md §6-6）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ToolGates {
    /// `Some(mode)` = 动态工作流已开启且带 mode（TS `isDynamicWorkflowModeEnabled(mode)`）。
    pub dynamic_workflow_mode: Option<String>,
}

/// TS `buildSessionCreateParams`：创建会话的运行时参数白名单。
///
/// 几条**不能简化**的规则（都来自 TS 注释）：
/// - `importedHistory` 是完整性边界，**不参与**兼容重试省略；
/// - `dynamicWorkflowEnabled` 只在为 `true` 时下发（关闭时不写字段）；
/// - `dynamicWorkflowMode` 只在与布尔同行时才写（单独出现对 CLI 无意义）。
pub fn session_create_params(params: &Value, gates: &ToolGates) -> Value {
    let mut request = Map::new();
    // TS 里 `sessionId`/`parentSessionId`/`mode`/`model` 无条件出现（undefined 由 JSON 序列化丢弃）。
    insert_optional(&mut request, "sessionId", text(params, "sessionId"));
    insert_optional(
        &mut request,
        "parentSessionId",
        text(params, "parentSessionId"),
    );
    insert_optional(&mut request, "mode", text(params, "mode"));
    insert_value(&mut request, "model", params.get("model"));
    insert_optional(&mut request, "persistence", text(params, "persistence"));
    insert_optional(&mut request, "thoughtLevel", text(params, "thoughtLevel"));
    insert_bool(
        &mut request,
        "titleGenerationEnabled",
        params,
        "titleGenerationEnabled",
    );
    insert_value(&mut request, "mcpServers", params.get("mcpServers"));
    insert_value(&mut request, "toolAllowlist", params.get("toolAllowlist"));
    insert_value(&mut request, "toolDenylist", params.get("toolDenylist"));
    // `importedHistory` 是导入历史的完整性边界：不参与兼容省略，有值即下发。
    insert_value(
        &mut request,
        "importedHistory",
        params.get("importedHistory"),
    );
    insert_gates(&mut request, gates);
    Value::Object(request)
}

/// TS `buildSessionResumeParams`：恢复会话的运行时参数白名单。
pub fn session_resume_params(params: &Value, gates: &ToolGates) -> Value {
    let mut request = Map::new();
    insert_optional(&mut request, "sessionId", text(params, "sessionId"));
    insert_optional(&mut request, "thoughtLevel", text(params, "thoughtLevel"));
    // 冷恢复时 app-server 可能重建 runtime；MCP 必须随 resume 下发，否则工具面会缺失。
    insert_value(&mut request, "mcpServers", params.get("mcpServers"));
    // 工具面约束必须和 create 路径一致随 resume 下发，否则冷恢复后会丢失 allow/deny 隔离。
    insert_value(&mut request, "toolAllowlist", params.get("toolAllowlist"));
    insert_value(&mut request, "toolDenylist", params.get("toolDenylist"));
    insert_gates(&mut request, gates);
    Value::Object(request)
}

/// `escode-session` 其余转发方法的运行时参数白名单（TS `escodeAgentService.ts` 各方法的
/// `client.request(method, {...})` 字面量；评审 A11：原实现把 Renderer 入参整包转发并附 `workspace`）。
///
/// 返回 `(运行时方法, 请求体)`；请求体逐字发出（`workspace` 引用只在 TS 也带的方法上由调用方补）。
pub fn session_forward_params(method: &str, params: &Value) -> Option<(&'static str, Value)> {
    let mut request = Map::new();
    let runtime_method = match method {
        "listSessions" => {
            // TS `includeArchived: params.includeArchived ?? false`：恒在场。
            request.insert(
                "includeArchived".to_owned(),
                Value::Bool(params.get("includeArchived").and_then(Value::as_bool) == Some(true)),
            );
            insert_value(&mut request, "limit", params.get("limit"));
            insert_value(&mut request, "sessionIds", params.get("sessionIds"));
            "session/list"
        }
        "readSession" => {
            insert_value(&mut request, "sessionId", params.get("sessionId"));
            for key in ["deliveryKind", "messageLimit", "afterSeq", "contentProfile"] {
                insert_value(&mut request, key, params.get(key));
            }
            "session/read"
        }
        "readSessionMessages" => {
            for key in ["sessionId", "afterMessageId", "limit"] {
                insert_value(&mut request, key, params.get(key));
            }
            "session/messages"
        }
        "closeSession" => {
            insert_value(&mut request, "sessionId", params.get("sessionId"));
            // TS 只在真值时带（`...(params.expectedPersistence ? {...} : {})`）。
            if let Some(expected) = text(params, "expectedPersistence").filter(|v| !v.is_empty()) {
                request.insert("expectedPersistence".to_owned(), Value::from(expected));
            }
            "session/close"
        }
        "setModel" => {
            for key in [
                "sessionId",
                "model",
                "expectedRevision",
                "persistAsWorkspaceLastUsed",
            ] {
                insert_value(&mut request, key, params.get(key));
            }
            "session/setModel"
        }
        "setThoughtLevel" => {
            for key in [
                "sessionId",
                "thoughtLevel",
                "expectedRevision",
                "persistAsWorkspaceLastUsed",
            ] {
                insert_value(&mut request, key, params.get(key));
            }
            "session/setThoughtLevel"
        }
        "setMode" => {
            for key in ["sessionId", "mode", "expectedRevision"] {
                insert_value(&mut request, key, params.get(key));
            }
            "session/setMode"
        }
        _ => return None,
    };
    Some((runtime_method, Value::Object(request)))
}

/// TS `withApiRetryRuntime` 的**无状态**部分（`escodeSessionApiRetry.ts:58`）：快照已带
/// `runtime.apiRetry` 时原样；会话终态（completed/error）补 `runtime.apiRetry: null`；其余原样。
///
/// 有状态部分（从 `session.updated`/`streamRecovery.updated` 事件跟踪进行中的重试并补回快照）
/// 依赖会话事件流，随事件面接入（登记差异：rust-u2-agent-v4.md §6）。
pub fn with_api_retry_runtime(mut snapshot: Value) -> Value {
    let has_identity = snapshot["session"]["sessionId"]
        .as_str()
        .is_some_and(|v| !v.is_empty())
        && snapshot["session"]["workspace"]["workspacePath"]
            .as_str()
            .is_some_and(|v| !v.is_empty());
    if !has_identity || snapshot["runtime"].get("apiRetry").is_some() {
        return snapshot;
    }
    let terminal = matches!(
        snapshot["session"]["status"].as_str(),
        Some("completed" | "error")
    );
    if terminal && let Some(record) = snapshot.as_object_mut() {
        let runtime = record
            .entry("runtime")
            .or_insert_with(|| Value::Object(Map::new()));
        if !runtime.is_object() {
            *runtime = Value::Object(Map::new());
        }
        if let Some(runtime) = runtime.as_object_mut() {
            runtime.insert("apiRetry".to_owned(), Value::Null);
        }
    }
    snapshot
}

/// TS `canReplayThoughtLevelFromResumeSnapshot` 前后的取值：请求值 trim 后非空；与快照不同且快照
/// 声明的可用档位里没有它时放弃重放（模型已切回 task-local 模型，旧档位可能不被支持）。
pub fn replayable_thought_level(params: &Value, snapshot: &Value) -> Option<String> {
    let requested = params["thoughtLevel"]
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())?
        .to_owned();
    if current_thought_level(snapshot) == Some(requested.as_str()) {
        return Some(requested);
    }
    match available_thought_levels(snapshot) {
        Some(available) if !available.contains(&requested) => None,
        _ => Some(requested),
    }
}

pub fn current_thought_level(snapshot: &Value) -> Option<&str> {
    snapshot["settings"]["thoughtLevel"]["current"].as_str()
}

/// TS `readSnapshotAvailableThoughtLevels`：`enabled=false` → 空集；`available` 为空或全无效 → None（不限制）。
fn available_thought_levels(snapshot: &Value) -> Option<Vec<String>> {
    let thought = &snapshot["settings"]["thoughtLevel"];
    if thought["enabled"] != true {
        return Some(Vec::new());
    }
    let values: Vec<String> = thought["available"]
        .as_array()?
        .iter()
        .filter_map(|option| {
            option
                .as_str()
                .or_else(|| option.get("value").and_then(Value::as_str))
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
        })
        .collect();
    (!values.is_empty()).then_some(values)
}

/// TS `appendWorkspaceToFilesystemMcpServers`：给 `filesystem` MCP 追加当前 workspace 参数。
///
/// 只在**本机路径存在**时注入（远程 workspace 不注入本机路径），且不修改用户配置——
/// 只改发往 runtime 的临时参数。无变化时原样返回（TS 用 `===` 判同一引用，这里按值判断）。
pub fn append_workspace_to_filesystem_mcp_servers(
    servers: Option<&Value>,
    workspace_path: &str,
) -> Option<Value> {
    let servers = servers.filter(|value| !value.is_null())?;
    let Some(items) = servers.as_array() else {
        return Some(servers.clone());
    };
    if items.is_empty() {
        return Some(servers.clone());
    }
    let workspace = workspace_path.trim();
    if workspace.is_empty() || !std::path::Path::new(workspace).exists() {
        return Some(servers.clone());
    }
    let workspace_key = normalize_path_for_compare(workspace);
    let mut changed = false;
    let next: Vec<Value> = items
        .iter()
        .map(|server| {
            if !is_filesystem_server(server) {
                return server.clone();
            }
            let has_workspace = server["args"].as_array().is_some_and(|args| {
                args.iter().any(|arg| {
                    arg.as_str()
                        .is_some_and(|value| normalize_path_for_compare(value) == workspace_key)
                })
            });
            if has_workspace {
                return server.clone();
            }
            changed = true;
            let mut updated = server.clone();
            if let Some(args) = updated["args"].as_array_mut() {
                args.push(Value::from(workspace));
            }
            updated
        })
        .collect();
    Some(if changed {
        Value::Array(next)
    } else {
        servers.clone()
    })
}

/// TS `isFilesystemServer`：`name === "filesystem"` 且 args 里带官方 server-filesystem 包。
fn is_filesystem_server(server: &Value) -> bool {
    server.get("command").is_some()
        && server.get("name").and_then(Value::as_str) == Some("filesystem")
        && server["args"].as_array().is_some_and(|args| {
            args.iter().any(|arg| {
                arg.as_str()
                    .is_some_and(|value| value.contains("@modelcontextprotocol/server-filesystem"))
            })
        })
}

/// TS `normalizePathForCompare`：`path.normalize` + 去尾部分隔符（Windows 再小写）。
///
/// 登记差异：Rust 侧不做 `path.normalize` 的 `..`/`.` 折叠（`std` 没有等价 API），
/// 只做去尾分隔符 + Windows 小写；差异只在「MCP args 里写了未折叠相对路径」时可见。
fn normalize_path_for_compare(value: &str) -> String {
    let trimmed = value.trim().replace(['\\', '/'], "/");
    let trimmed = trimmed.trim_end_matches('/').to_owned();
    if cfg!(windows) {
        trimmed.to_lowercase()
    } else {
        trimmed
    }
}

/// 灰度字段：只在命中时下发 `true`（关闭时不写字段，CLI 缺省即不注册对应工具）；
/// `dynamicWorkflowMode` 只与布尔同行。
fn insert_gates(request: &mut Map<String, Value>, gates: &ToolGates) {
    if let Some(mode) = &gates.dynamic_workflow_mode {
        request.insert("dynamicWorkflowEnabled".to_owned(), Value::Bool(true));
        request.insert("dynamicWorkflowMode".to_owned(), Value::from(mode.clone()));
    }
}

fn insert_bool(request: &mut Map<String, Value>, key: &str, params: &Value, source: &str) {
    if let Some(value) = params.get(source).and_then(Value::as_bool) {
        request.insert(key.to_owned(), Value::Bool(value));
    }
}

fn insert_optional(request: &mut Map<String, Value>, key: &str, value: Option<String>) {
    if let Some(value) = value {
        request.insert(key.to_owned(), Value::from(value));
    }
}

fn insert_value(request: &mut Map<String, Value>, key: &str, value: Option<&Value>) {
    if let Some(value) = value.filter(|value| !value.is_null()) {
        request.insert(key.to_owned(), value.clone());
    }
}

fn text(params: &Value, key: &str) -> Option<String> {
    params
        .get(key)
        .filter(|value| !value.is_null())
        .and_then(Value::as_str)
        .map(str::to_owned)
}

#[cfg(test)]
#[path = "session_params_tests.rs"]
mod tests;
