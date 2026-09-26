//! MCP OAuth 凭据记录（docs/specs/rust-mcp-oauth.md 第 2 层），对齐 TS `oauth-credentials.ts`、
//! `oauth-shared.ts`（discovery 缓存）与 `oauth-lease.ts`（pending 授权）；存储为与 Node 共用的加密凭据文件。
use crate::domain::mcp_oauth::{self as rules, Pair};
use anyhow::Result;
use serde_json::{Value, json};
use zcode_cli_host::credential_store::CredentialStore;

/// discovery 缓存寿命（TS `MCP_OAUTH_DISCOVERY_TTL_MS`）。
const DISCOVERY_TTL_MS: u64 = 24 * 60 * 60 * 1000;

pub(super) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// TS `loadCredentialPair`。
pub(super) async fn load_pair(store: &CredentialStore, prefix: &str) -> Result<Option<Pair>> {
    let keys = [
        rules::key(prefix, rules::CANONICAL_KEY),
        rules::key(prefix, rules::LEGACY_CLIENT_KEY),
        rules::key(prefix, rules::LEGACY_TOKENS_KEY),
    ];
    let values = store
        .load_many(&keys.iter().map(String::as_str).collect::<Vec<_>>())
        .await?;
    let get = |k: &String| values.get(k).cloned().flatten();
    Ok(rules::derive_pair(
        get(&keys[0]).as_deref(),
        get(&keys[1]).as_deref(),
        get(&keys[2]).as_deref(),
    ))
}

/// TS `loadCanonicalCredentials`：只读 canonical，generation 用于观察换代。
pub(super) async fn load_canonical(store: &CredentialStore, prefix: &str) -> Result<Option<Pair>> {
    let raw = store.load(&rules::key(prefix, rules::CANONICAL_KEY)).await?;
    Ok(raw
        .as_deref()
        .and_then(|raw| rules::derive_pair(Some(raw), None, None))
        .filter(|pair| pair.canonical))
}

/// TS `publishCanonicalCredentials`：canonical 与 legacy 镜像在同一次锁内写入。
pub(super) async fn publish(
    store: &CredentialStore,
    prefix: &str,
    client: &Value,
    tokens: &Value,
    issuer: Option<&str>,
    published_by: &str,
) -> Result<String> {
    let obtained = now_ms();
    let generation = hex(&zcode_cli_host::credential_cipher::random_bytes(16));
    // serde_json 按键名排序输出，与 TS 对 canonical 的字面顺序一致。
    let mut canonical = json!({
        "client_information": client,
        "generation": generation,
        "obtained_at": obtained,
        "published_by": published_by,
        "tokens": tokens,
        "version": 2,
    });
    if let Some(seconds) = tokens["expires_in"].as_f64().filter(|s| s.is_finite()) {
        canonical["expires_at"] = json!(obtained as f64 + seconds * 1000.0);
    }
    if let Some(issuer) = issuer {
        canonical["issuer"] = issuer.into();
    }
    store
        .save_many(&[
            (rules::key(prefix, rules::CANONICAL_KEY), canonical.to_string()),
            (rules::key(prefix, rules::LEGACY_CLIENT_KEY), client.to_string()),
            (rules::key(prefix, rules::LEGACY_TOKENS_KEY), tokens.to_string()),
        ])
        .await?;
    Ok(generation)
}

/// TS `invalidateCanonicalCredentials`：canonical 仍是 `expected_raw` 才整体删除（`all` 同时删 client）。
pub(super) async fn invalidate(
    store: &CredentialStore,
    prefix: &str,
    expected_raw: &str,
    all: bool,
) -> Result<bool> {
    let canonical = rules::key(prefix, rules::CANONICAL_KEY);
    let mut keys = vec![canonical.clone(), rules::key(prefix, rules::LEGACY_TOKENS_KEY)];
    if all {
        keys.push(rules::key(prefix, rules::LEGACY_CLIENT_KEY));
    }
    store.delete_many_if_value(&canonical, expected_raw, &keys).await
}

/// TS `loadDiscoveryRecord`：缺时间戳、过期、解析失败或 issuer 不符均视为无缓存。
pub(super) async fn load_discovery(
    store: &CredentialStore,
    prefix: &str,
    expected_issuer: Option<&str>,
) -> Result<Option<Value>> {
    let state_key = rules::key(prefix, rules::DISCOVERY_STATE_KEY);
    let fetched_key = rules::key(prefix, rules::DISCOVERY_FETCHED_AT_KEY);
    let values = store.load_many(&[&state_key, &fetched_key]).await?;
    let Some(raw) = values.get(&state_key).cloned().flatten() else {
        return Ok(None);
    };
    let fetched = values
        .get(&fetched_key)
        .cloned()
        .flatten()
        .and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|v| v.is_finite());
    let Some(fetched) = fetched else {
        return Ok(None);
    };
    if now_ms() as f64 - fetched >= DISCOVERY_TTL_MS as f64 {
        return Ok(None);
    }
    let Some(state) = serde_json::from_str::<Value>(&raw)
        .ok()
        .filter(|v| v["authorizationServerUrl"].is_string())
    else {
        return Ok(None);
    };
    if let Some(expected) = expected_issuer {
        let cached = state["authorizationServerMetadata"]["issuer"]
            .as_str()
            .or(state["authorizationServerUrl"].as_str())
            .unwrap_or_default();
        if cached.trim_end_matches('/') != expected.trim_end_matches('/') {
            return Ok(None);
        }
    }
    Ok(Some(state))
}
/// TS `saveDiscoveryRecord`：状态与时间戳分两个 key（旧 reader 只认裸状态）。
pub(super) async fn save_discovery(store: &CredentialStore, prefix: &str, state: &Value) -> Result<()> {
    store
        .save_many(&[
            (rules::key(prefix, rules::DISCOVERY_STATE_KEY), state.to_string()),
            (rules::key(prefix, rules::DISCOVERY_FETCHED_AT_KEY), now_ms().to_string()),
        ])
        .await
}

/// TS `publishPendingAuthorization`。
pub(super) async fn publish_pending(
    store: &CredentialStore,
    prefix: &str,
    attempt_id: &str,
    authorization_url: &str,
    baseline_generation: Option<&str>,
    expires_at: u64,
    state: &str,
) -> Result<()> {
    let mut record = json!({
        "attempt_id": attempt_id,
        "authorization_url": authorization_url,
        "expires_at": expires_at,
        "state": state,
    });
    if let Some(generation) = baseline_generation {
        record["baseline_generation"] = generation.into();
    }
    store
        .save(&rules::key(prefix, rules::PENDING_KEY), &record.to_string())
        .await
}
/// TS `loadPendingAuthorization`：过期视为不存在；返回共享的授权 URL。
pub(super) async fn load_pending(store: &CredentialStore, prefix: &str) -> Result<Option<String>> {
    let Some(raw) = store.load(&rules::key(prefix, rules::PENDING_KEY)).await? else {
        return Ok(None);
    };
    let Ok(value) = serde_json::from_str::<Value>(&raw) else {
        return Ok(None);
    };
    let (Some(_), Some(url), Some(_), Some(expires)) = (
        value["attempt_id"].as_str(),
        value["authorization_url"].as_str(),
        value["state"].as_str(),
        value["expires_at"].as_f64(),
    ) else {
        return Ok(None);
    };
    if expires <= now_ms() as f64 {
        return Ok(None);
    }
    Ok(Some(url.to_owned()))
}
/// TS `deletePendingAuthorizationIfOwned`：只删本 attempt 发布的 pending（按原始值 CAS）。
pub(super) async fn delete_pending_if_owned(
    store: &CredentialStore,
    prefix: &str,
    attempt_id: &str,
) -> Result<bool> {
    let key = rules::key(prefix, rules::PENDING_KEY);
    let Some(raw) = store.load(&key).await? else {
        return Ok(false);
    };
    match serde_json::from_str::<Value>(&raw) {
        Err(_) => store.delete_if_value(&key, &raw).await,
        Ok(value) if value["attempt_id"] == attempt_id => store.delete_if_value(&key, &raw).await,
        Ok(_) => Ok(false),
    }
}
