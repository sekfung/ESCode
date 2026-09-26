use serde_json::Value;
/// TS `mcpRequestMeta`：请求上下文以扁平键与 `com.zcode/request-context` 两份下发，每次调用一个新 span_id。
/// node_repl 的浏览器 bridge 以它回到当前会话（docs/specs/rust-browser-use.md 第 1 期）。
pub(super) fn request_meta(base: &Value) -> Option<Value> {
    let base = base.as_object().filter(|m| !m.is_empty())?;
    let span: String = crate::id().chars().take(16).collect();
    let order = [
        "trace_id",
        "span_id",
        "parent_span_id",
        "session_id",
        "turn_id",
        "runtime_scope",
        "workspace_path",
        "workspace_identity",
        "workspace_key",
        "client_mode",
        "delivery_kind",
    ];
    let mut context = serde_json::Map::new();
    for key in order {
        let value = if key == "span_id" { Some(Value::from(span.clone())) } else { base.get(key).cloned() };
        if let Some(value) = value {
            context.insert(key.into(), value);
        }
    }
    let mut meta = context.clone();
    meta.insert("com.zcode/request-context".into(), Value::Object(context));
    Some(Value::Object(meta))
}
