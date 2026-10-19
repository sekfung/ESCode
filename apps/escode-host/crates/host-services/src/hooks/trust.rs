//! Workspace Hook Trust store 读取（TS `workspace-hook-trust-store-file.ts` 的 strict schema）。
//!
//! 信任存储是权限边界：文件不可读、JSON 语法错误或 schema 不合法时一律 fail-closed，
//! 不返回任何部分 digest；只有 ENOENT 表示“没有记录”而非损坏。

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::config::HookEvent;
use super::paths;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TrustRead {
    pub digests: HashSet<String>,
    pub corrupt: bool,
}

/// 读取 `<storageRoot>/security/workspace-hook-trust-v1.json` 并过滤 `workspaceIdentity`。
pub async fn read(home: &Path, workspace_identity: &str) -> TrustRead {
    let user_config =
        super::config::read_json_object(&paths::join(home, &[".escode", "cli", "config.json"]))
            .await;
    let storage_root = storage_root(home, user_config.as_ref());
    let path = storage_root
        .join("security")
        .join("workspace-hook-trust-v1.json");
    let text = match tokio::fs::read_to_string(&path).await {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return TrustRead::default();
        }
        Err(error) => {
            escode_host_control::logging::warn(
                "hooks-service",
                format!(
                    "Workspace Hook Trust store 不可读，已 fail-closed 忽略全部持久信任记录: {}: {error}",
                    paths::path_text(&path)
                ),
            );
            return TrustRead {
                digests: HashSet::new(),
                corrupt: true,
            };
        }
    };
    match parse(&text) {
        Some(records) => TrustRead {
            digests: records
                .into_iter()
                .filter(|record| record.workspace_identity == workspace_identity)
                .map(|record| record.digest)
                .collect(),
            corrupt: false,
        },
        None => {
            escode_host_control::logging::warn(
                "hooks-service",
                format!(
                    "Workspace Hook Trust store 结构不符合 schema，已 fail-closed 忽略全部持久信任记录: {}",
                    paths::path_text(&path)
                ),
            );
            TrustRead {
                digests: HashSet::new(),
                corrupt: true,
            }
        }
    }
}

fn storage_root(home: &Path, user_config: Option<&serde_json::Map<String, Value>>) -> PathBuf {
    let configured = user_config
        .and_then(|config| config.get("storage"))
        .and_then(Value::as_object)
        .and_then(|storage| storage.get("dir"))
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    if configured.is_empty() {
        return paths::normalize(&home.join(".escode"));
    }
    if let Some(rest) = configured.strip_prefix("~/") {
        return paths::normalize(&home.join(rest));
    }
    let path = Path::new(configured);
    if path.is_absolute() {
        paths::normalize(path)
    } else {
        paths::normalize(&home.join(path))
    }
}

struct Record {
    workspace_identity: String,
    digest: String,
}

fn parse(text: &str) -> Option<Vec<Record>> {
    let value: Value = serde_json::from_str(text).ok()?;
    let root = value.as_object()?;
    if root.len() != 2 || !root.contains_key("schemaVersion") || !root.contains_key("records") {
        return None;
    }
    if root["schemaVersion"].as_u64() != Some(1) {
        return None;
    }
    let records = root["records"].as_array()?;
    let mut parsed = Vec::with_capacity(records.len());
    let mut keys = HashSet::new();
    for record in records {
        let record = parse_record(record)?;
        if !keys.insert(format!(
            "{}\u{0}{}",
            record.workspace_identity, record.digest
        )) {
            return None;
        }
        parsed.push(record);
    }
    Some(parsed)
}

fn parse_record(value: &Value) -> Option<Record> {
    let object = value.as_object()?;
    const ALLOWED: [&str; 15] = [
        "workspaceIdentity",
        "hookDeclarationDigest",
        "digestAlgorithm",
        "decision",
        "grantedAt",
        "lastUsedAt",
        "bundleDigestAtGrant",
        "eventAtGrant",
        "displayCommandAtGrant",
        "sourcePathAtGrant",
        "sourceDiscoveryOrderAtGrant",
        "matcherAtGrant",
        "matcherIndexAtGrant",
        "hookIndexAtGrant",
        "appVersionAtGrant",
    ];
    if object.keys().any(|key| !ALLOWED.contains(&key.as_str())) {
        return None;
    }
    let workspace_identity = trimmed_non_empty(object.get("workspaceIdentity")?)?;
    let digest = object.get("hookDeclarationDigest")?.as_str()?;
    if !sha256_hex(digest) {
        return None;
    }
    if object.get("digestAlgorithm")?.as_str()? != "sha256"
        || object.get("decision")?.as_str()? != "trusted"
    {
        return None;
    }
    if !datetime(object.get("grantedAt")?.as_str()?) {
        return None;
    }
    if let Some(value) = object.get("lastUsedAt")
        && !datetime(value.as_str()?)
    {
        return None;
    }
    if let Some(value) = object.get("bundleDigestAtGrant")
        && !sha256_hex(value.as_str()?)
    {
        return None;
    }
    HookEvent::parse(object.get("eventAtGrant")?.as_str()?)?;
    trimmed_non_empty(object.get("displayCommandAtGrant")?)?;
    trimmed_non_empty(object.get("sourcePathAtGrant")?)?;
    if let Some(value) = object.get("sourceDiscoveryOrderAtGrant")
        && !nonnegative_integer(value)
    {
        return None;
    }
    if let Some(value) = object.get("matcherAtGrant")
        && !value.is_null()
        && !value.is_string()
    {
        return None;
    }
    if let Some(value) = object.get("matcherIndexAtGrant")
        && !nonnegative_integer(value)
    {
        return None;
    }
    if let Some(value) = object.get("hookIndexAtGrant")
        && !nonnegative_integer(value)
    {
        return None;
    }
    if let Some(value) = object.get("appVersionAtGrant") {
        trimmed_non_empty(value)?;
    }
    Some(Record {
        workspace_identity,
        digest: digest.to_owned(),
    })
}

fn trimmed_non_empty(value: &Value) -> Option<String> {
    let text = value.as_str()?.trim();
    if text.is_empty() {
        None
    } else {
        Some(text.to_owned())
    }
}

fn nonnegative_integer(value: &Value) -> bool {
    value.as_u64().is_some()
}

fn sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// zod `.datetime()` 的 RFC3339 子集：`YYYY-MM-DDTHH:MM:SS[.fraction](Z|±HH:MM|空)`。
fn datetime(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() < 19
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes[10] != b'T'
        || bytes[13] != b':'
        || bytes[16] != b':'
    {
        return false;
    }
    let number =
        |range: std::ops::Range<usize>| -> Option<u32> { value.get(range)?.parse::<u32>().ok() };
    let (Some(year), Some(month), Some(day), Some(hour), Some(minute), Some(second)) = (
        number(0..4),
        number(5..7),
        number(8..10),
        number(11..13),
        number(14..16),
        number(17..19),
    ) else {
        return false;
    };
    if year == 0
        || !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return false;
    }
    let mut rest = &bytes[19..];
    if rest.first() == Some(&b'.') {
        let digits = rest[1..]
            .iter()
            .take_while(|byte| byte.is_ascii_digit())
            .count();
        if digits == 0 {
            return false;
        }
        rest = &rest[1 + digits..];
    }
    match rest {
        [] | [b'Z'] | [b'z'] => true,
        [sign, rest @ ..] if *sign == b'+' || *sign == b'-' => {
            rest.len() == 5
                && rest[0].is_ascii_digit()
                && rest[1].is_ascii_digit()
                && rest[2] == b':'
                && rest[3].is_ascii_digit()
                && rest[4].is_ascii_digit()
                && rest[0..2]
                    .iter()
                    .fold(0u32, |acc, byte| acc * 10 + u32::from(byte - b'0'))
                    <= 23
                && rest[3..5]
                    .iter()
                    .fold(0u32, |acc, byte| acc * 10 + u32::from(byte - b'0'))
                    <= 59
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn record(identity: &str, digest: &str) -> Value {
        serde_json::json!({
            "workspaceIdentity": identity,
            "hookDeclarationDigest": digest,
            "digestAlgorithm": "sha256",
            "decision": "trusted",
            "grantedAt": "2026-10-19T10:00:00.000Z",
            "eventAtGrant": "PreToolUse",
            "displayCommandAtGrant": "node hook.mjs",
            "sourcePathAtGrant": ".escode/config.json"
        })
    }

    #[test]
    fn parses_typescript_written_store_and_trims_identity() {
        let parsed = parse(
            &serde_json::json!({
                "schemaVersion": 1,
                "records": [record("  ws-a  ", DIGEST)]
            })
            .to_string(),
        )
        .unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].workspace_identity, "ws-a");
    }

    #[test]
    fn rejects_unknown_keys_bad_digests_duplicates_and_bad_datetimes() {
        let mut unknown = record("ws", DIGEST);
        unknown["extra"] = serde_json::json!(true);
        let cases = [
            unknown,
            record("ws", &DIGEST.to_uppercase()),
            record("ws", "short"),
            serde_json::json!({
                "workspaceIdentity": "ws",
                "hookDeclarationDigest": DIGEST,
                "digestAlgorithm": "sha256",
                "decision": "trusted",
                "grantedAt": "2026-13-19T10:00:00.000Z",
                "eventAtGrant": "PreToolUse",
                "displayCommandAtGrant": "x",
                "sourcePathAtGrant": "y"
            }),
        ];
        for case in cases {
            assert!(
                parse(&serde_json::json!({"schemaVersion": 1, "records": [case]}).to_string())
                    .is_none()
            );
        }
        let duplicate = serde_json::json!({
            "schemaVersion": 1,
            "records": [record("ws", DIGEST), record("ws", DIGEST)]
        });
        assert!(parse(&duplicate.to_string()).is_none());
    }

    #[test]
    fn storage_root_follows_the_typescript_resolution_rules() {
        let home = Path::new("/home/u");
        assert_eq!(storage_root(home, None), PathBuf::from("/home/u/.escode"));
        let config = serde_json::json!({"storage": {"dir": "~/data"}});
        assert_eq!(
            storage_root(home, config.as_object()),
            PathBuf::from("/home/u/data")
        );
        let config = serde_json::json!({"storage": {"dir": "rel"}});
        assert_eq!(
            storage_root(home, config.as_object()),
            PathBuf::from("/home/u/rel")
        );
    }
}
