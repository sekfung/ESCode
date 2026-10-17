//! `gen-ui` 的 schema 校验（TS `packages/shared/src/gen-ui/contract.ts` 的 zod schema）。
//!
//! 逐条复刻判定口径：**strict**（未声明键即失败）、长度按 **UTF-16 码元**（zod 的 `.max()` 口径）、
//! 缺省值 `null`、16 KiB 按序列化后的 **UTF-8 字节**。
//! 登记差异：zod 失败时抛的是 `ZodError`（issue JSON），Rust 只回一句话——**只比接受/拒绝，不比文案**
//! （与 `plugin_result_schema` 同口径，见 `rust-u2-gen-ui.md` §5）。

use serde_json::{Map, Value};

/// TS `GEN_UI_STATE_MAX_BYTES`。
pub const STATE_MAX_BYTES: usize = 16 * 1024;
/// TS `MAX_RECORD_BYTES`（stateStorage）。
pub const RECORD_MAX_BYTES: u64 = 96 * 1024;
/// TS `GEN_UI_STATE_MAX_ENTRIES`。
pub const STATE_MAX_ENTRIES: usize = 100;

/// JS `String.length` / zod `.max()` 的口径：UTF-16 码元数。
fn utf16_len(value: &str) -> usize {
    value.chars().map(char::len_utf16).sum()
}

/// TS `genUiScopeSchema`（strict）。
#[derive(Clone, Debug, PartialEq)]
pub struct Scope {
    pub workspace_path: String,
    pub workspace_identity: Option<String>,
    pub session_id: String,
}

/// TS `genUiStateTargetSchema`（strict）。
#[derive(Clone, Debug, PartialEq)]
pub struct Target {
    pub scope: Scope,
    pub path: String,
}

/// strict 检查：对象里不得出现白名单之外的键（zod `.strict()`）。
fn reject_unknown_keys(object: &Map<String, Value>, allowed: &[&str]) -> Result<(), String> {
    if let Some(extra) = object.keys().find(|key| !allowed.contains(&key.as_str())) {
        return Err(format!("Unrecognized key: \"{extra}\""));
    }
    Ok(())
}

/// zod 的可选字符串段：键缺省可，**`null` 不可**。
fn bounded_string(
    object: &Map<String, Value>,
    key: &str,
    min: usize,
    max: usize,
) -> Result<String, String> {
    let value = object.get(key).ok_or_else(|| format!("{key}: Required"))?;
    let text = value
        .as_str()
        .ok_or_else(|| format!("{key}: Expected string"))?;
    let length = utf16_len(text);
    if length < min {
        return Err(format!(
            "{key}: String must contain at least {min} character(s)"
        ));
    }
    if length > max {
        return Err(format!(
            "{key}: String must contain at most {max} character(s)"
        ));
    }
    Ok(text.to_owned())
}

/// TS `genUiPathSchema`：绝对路径 + 无 `..` 段 + 末段是小写连字符 `.html`。
///
/// 这条 schema 同时是**安全边界的前置**：过了它就保证路径里没有 `..` 段，
/// 后续的归一化只需处理 `.` 与重复分隔符（见 `normalize_posix`）。
pub fn is_valid_document_path(path: &str) -> bool {
    let length = utf16_len(path);
    if length == 0 || length > 4096 {
        return false;
    }
    if path.contains('\0') || path.contains('\r') || path.contains('\n') {
        return false;
    }
    let segments: Vec<&str> = path.split(['\\', '/']).collect();
    if segments.contains(&"..") {
        return false;
    }
    let rooted = path.starts_with('/')
        || path.starts_with("\\\\")
        || (path.len() >= 3
            && path.as_bytes()[1] == b':'
            && (path.as_bytes()[2] == b'\\' || path.as_bytes()[2] == b'/')
            && path.as_bytes()[0].is_ascii_alphabetic());
    if !rooted {
        return false;
    }
    let last = segments.last().copied().unwrap_or_default();
    is_lowercase_hyphenated_html(last)
}

fn is_lowercase_hyphenated_html(name: &str) -> bool {
    let Some(stem) = name.strip_suffix(".html") else {
        return false;
    };
    if stem.is_empty() {
        return false;
    }
    let parts: Vec<&str> = stem.split('-').collect();
    if parts.iter().any(|part| part.is_empty()) {
        return false;
    }
    // `^[a-z0-9]+(?:-[a-z0-9]+)*$`：每段都非空且只含小写字母与数字。
    parts.iter().all(|part| {
        part.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
    })
}

pub fn parse_scope(raw: &Value) -> Result<Scope, String> {
    let object = raw
        .as_object()
        .ok_or_else(|| "Expected object".to_owned())?;
    reject_unknown_keys(object, &["workspacePath", "workspaceIdentity", "sessionId"])?;
    parse_scope_fields(object)
}

/// scope 的字段解析（不含 strict 键检查）：`genUiStateTargetSchema` 是 `scope.extend({path})`，
/// 故 `path` 在 target 的 strict 检查里放行，这里不能重复拒绝。
fn parse_scope_fields(object: &Map<String, Value>) -> Result<Scope, String> {
    let workspace_identity = match object.get("workspaceIdentity") {
        None => None,
        Some(value) => {
            let text = value
                .as_str()
                .ok_or_else(|| "workspaceIdentity: Expected string".to_owned())?;
            if utf16_len(text) > 4096 {
                return Err("workspaceIdentity: too long".to_owned());
            }
            Some(text.to_owned())
        }
    };
    Ok(Scope {
        workspace_path: bounded_string(object, "workspacePath", 1, 4096)?,
        workspace_identity,
        session_id: bounded_string(object, "sessionId", 1, 512)?,
    })
}

pub fn parse_target(raw: &Value) -> Result<Target, String> {
    let object = raw
        .as_object()
        .ok_or_else(|| "Expected object".to_owned())?;
    reject_unknown_keys(
        object,
        &["workspacePath", "workspaceIdentity", "sessionId", "path"],
    )?;
    let scope = parse_scope_fields(object)?;
    let path = bounded_string(object, "path", 1, 4096)?;
    if !is_valid_document_path(&path) {
        return Err(
            "Expected an absolute executor path to a lowercase-hyphenated HTML file".to_owned(),
        );
    }
    Ok(Target { scope, path })
}

/// TS `genUiWidgetStateSchema`：strict 的两键、缺省 `null`、序列化后 ≤ 16 KiB。
/// 返回值是**规范化后的**状态（键集合固定为 `modelContent` + `privateContent`）。
pub fn parse_widget_state(raw: &Value) -> Result<Value, String> {
    let object = raw
        .as_object()
        .ok_or_else(|| "Expected object".to_owned())?;
    reject_unknown_keys(object, &["modelContent", "privateContent"])?;
    let mut state = Map::new();
    for key in ["modelContent", "privateContent"] {
        state.insert(
            key.to_owned(),
            object.get(key).cloned().unwrap_or(Value::Null),
        );
    }
    let state = Value::Object(state);
    let serialized = serde_json::to_string(&state)
        .map_err(|error| format!("Widget state must contain JSON values only: {error}"))?;
    if serialized.len() > STATE_MAX_BYTES {
        return Err("Widget state exceeds 16 KiB".to_owned());
    }
    Ok(state)
}

/// TS `buildGenUiScopeKey` = `JSON.stringify([identity?.trim() || workspacePath, sessionId])`。
/// **数组**（键序固定）；字符串转义由 `serde_json` 与 JS 同口径，故可直接当 hash 输入。
pub fn scope_key(scope: &Scope) -> String {
    let first = scope
        .workspace_identity
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(&scope.workspace_path);
    // 数组序列化不会失败（元素都是字符串）。
    serde_json::to_string(&[first, scope.session_id.as_str()])
        .unwrap_or_else(|_| format!("[\"{first}\",\"{}\"]", scope.session_id))
}

#[cfg(test)]
#[path = "schema_tests.rs"]
mod tests;
