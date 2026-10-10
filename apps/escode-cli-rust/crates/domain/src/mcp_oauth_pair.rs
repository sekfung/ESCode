//! MCP OAuth 凭据 pair 派生（TS `oauth-credentials.ts` 的 `deriveCredentialPair` 与 canonical 记录规则）。
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// TS `CredentialPairSnapshot`。
#[derive(Clone, Debug, PartialEq)]
pub struct Pair {
    pub client: Option<Value>,
    pub tokens: Option<Value>,
    pub generation: Option<String>,
    pub raw: Option<String>,
    pub expires_at: Option<f64>,
    pub obtained_at: Option<f64>,
    pub issuer: Option<String>,
    pub canonical: bool,
}
impl Pair {
    /// 与 TS 对象同形的 JSON（语料比对用）。
    pub fn to_json(&self) -> Value {
        let mut value = json!({"source": if self.canonical {"canonical"} else {"legacy"}});
        for (key, field) in [("clientInformation", &self.client), ("tokens", &self.tokens)] {
            if let Some(v) = field {
                value[key] = v.clone();
            }
        }
        for (key, field) in [("generation", &self.generation), ("raw", &self.raw), ("issuer", &self.issuer)] {
            if let Some(v) = field {
                value[key] = v.clone().into();
            }
        }
        for (key, field) in [("expiresAt", self.expires_at), ("obtainedAt", self.obtained_at)] {
            if let Some(v) = field {
                value[key] = json!(v);
            }
        }
        value
    }
    /// TS `isCanonicalTokenNearExpiry`：无过期点视为临期；30s 余量。
    pub fn near_expiry(&self, now_ms: f64) -> bool {
        self.expires_at.is_none_or(|at| now_ms >= at - 30_000.0)
    }
    pub fn access_token(&self) -> Option<&str> {
        self.tokens.as_ref()?["access_token"].as_str()
    }
    pub fn refresh_token(&self) -> Option<&str> {
        self.tokens.as_ref()?["refresh_token"].as_str()
    }
}

/// TS `isCanonicalCredentials`。
fn is_canonical(value: &Value) -> bool {
    matches!(value["version"].as_f64(), Some(v) if v == 1.0 || v == 2.0)
        && value["published_by"].as_str().is_some_and(|s| !s.is_empty())
        && value["client_information"]["client_id"].is_string()
        && value["tokens"]["access_token"].is_string()
        && value["tokens"]["token_type"].is_string()
}
/// TS `resolveCredentialGeneration`。
fn generation(canonical: &Value, raw: &str) -> String {
    match canonical["generation"].as_str().filter(|g| !g.is_empty()) {
        Some(generation) => generation.to_owned(),
        None => format!("legacy-{}", &hex(&Sha256::digest(raw.as_bytes()))[..32]),
    }
}
fn parse(raw: Option<&str>) -> Option<Value> {
    serde_json::from_str(raw?).ok()
}

/// TS `deriveCredentialPair`（兼容窗口内 canonical 与 legacy 镜像的派生规则）。
pub fn derive_pair(
    canonical_raw: Option<&str>,
    legacy_client_raw: Option<&str>,
    legacy_tokens_raw: Option<&str>,
) -> Option<Pair> {
    let legacy_client = parse(legacy_client_raw);
    let legacy_tokens = parse(legacy_tokens_raw);
    let legacy = |client: Option<Value>, tokens: Option<Value>| Pair {
        client,
        tokens,
        generation: None,
        raw: None,
        expires_at: None,
        obtained_at: None,
        issuer: None,
        canonical: false,
    };
    if let (Some(raw), Some(canonical)) = (canonical_raw, parse(canonical_raw).filter(is_canonical)) {
        let snapshot = Pair {
            client: Some(canonical["client_information"].clone()),
            tokens: Some(canonical["tokens"].clone()),
            generation: Some(generation(&canonical, raw)),
            raw: Some(raw.to_owned()),
            expires_at: canonical["expires_at"].as_f64(),
            obtained_at: canonical["obtained_at"].as_f64(),
            issuer: canonical["issuer"].as_str().map(str::to_owned),
            canonical: true,
        };
        if canonical["version"].as_f64() == Some(1.0) {
            let adopt = legacy_tokens.is_some()
                && legacy_client
                    .as_ref()
                    .is_none_or(|c| *c == canonical["client_information"]);
            if adopt {
                return Some(legacy(
                    legacy_client.or(Some(canonical["client_information"].clone())),
                    legacy_tokens,
                ));
            }
            return Some(snapshot);
        }
        let Some(tokens) = legacy_tokens else {
            return Some(legacy(legacy_client, None));
        };
        if tokens == canonical["tokens"] {
            return Some(if legacy_client.is_some() {
                snapshot
            } else {
                legacy(None, Some(tokens))
            });
        }
        if legacy_client
            .as_ref()
            .is_some_and(|c| *c == canonical["client_information"])
        {
            return Some(legacy(legacy_client, Some(tokens)));
        }
        return Some(legacy(legacy_client, None));
    }
    if legacy_client.is_none() && legacy_tokens.is_none() {
        return None;
    }
    Some(legacy(legacy_client, legacy_tokens))
}

