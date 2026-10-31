//! 一次性 capability 的签发与消费（HTTP `/api/rpc-host-capability`，spec §10.11）。
//!
//! 不带 `scope` 的请求与旧行为一致：签发本地 scope。带 `scope` 时**先**建立/复用该 profile 的
//! 远端连接（S3-a single-flight + `Initialize`），成功后才签发绑定该客户端句柄的 capability；
//! 任何失败都返回显式错误并且不签发——绝不下发可回退到本地权限的令牌。

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::remote_scope::{RemoteGrant, RemoteScope};
use crate::session::Channels;

/// 一次性 capability 的授权内容：未带远端 scope 即本地 scope（现有行为）。
#[derive(Clone, Default)]
pub struct CapabilityGrant {
    pub remote: Option<RemoteGrant>,
}

/// capability 表：一次性签发 + TTL（与 TS `createHostCapabilityStore` 语义一致的最小实现）。
#[derive(Default)]
pub struct Capabilities {
    tokens: std::sync::Mutex<BTreeMap<String, (Instant, CapabilityGrant)>>,
    ttl: Duration,
}

impl Capabilities {
    pub fn new(ttl: Duration) -> Self {
        Self {
            tokens: Default::default(),
            ttl,
        }
    }

    /// 签发一次性 capability。
    ///
    /// 修复（评审 A4）：旧令牌是「纳秒 ‖ pid」，可预测；改为 CSPRNG（两段 uuid v4，244 位随机）。
    pub fn issue(&self) -> String {
        self.issue_grant(CapabilityGrant::default())
    }

    /// 签发绑定授权内容的一次性 capability（远端 scope 在此绑定连接句柄）。
    pub fn issue_grant(&self, grant: CapabilityGrant) -> String {
        let token = random_token();
        let mut tokens = self.lock();
        let now = Instant::now();
        tokens.retain(|_, (expires, _)| *expires > now);
        tokens.insert(token.clone(), (now + self.ttl, grant));
        token
    }

    /// 消费一次（成功即失效，返回绑定时的授权内容）。
    pub fn consume(&self, token: Option<&str>) -> Option<CapabilityGrant> {
        let token = token?;
        let mut tokens = self.lock();
        let now = Instant::now();
        tokens.retain(|_, (expires, _)| *expires > now);
        tokens.remove(token).map(|(_, grant)| grant)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, (Instant, CapabilityGrant)>> {
        self.tokens
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// 按 capability 请求 body 签发：`{"scope": {...}}` 绑定远端 scope，其余为本地 scope。
///
/// 远端 scope 的连接失败（未知 profile / transport / 握手超时）原样上抛，调用方回 4xx 且不签发。
pub async fn issue_for(
    channels: &Channels,
    capabilities: &Capabilities,
    body: &[u8],
) -> Result<String, String> {
    let Some(scope) = parse_scope(body)? else {
        return Ok(capabilities.issue());
    };
    let support = channels
        .remote_scope()
        .ok_or_else(|| "this Host does not serve remote workspaces".to_owned())?;
    // 组合根注入的 ensure 承诺「Initialize 完成」；签发即绑定该客户端句柄。
    let client = (support.ensure)(scope.clone()).await?;
    // 生命周期事件（一次性）：capability 绑定的是哪个 profile/source，不含用户数据。
    escode_host_control::logging::info(
        "source-server",
        format!(
            "remote scope capability issued for {}",
            crate::remote_scope::scope_log_value(&scope)
        ),
    );
    Ok(capabilities.issue_grant(CapabilityGrant {
        remote: Some(RemoteGrant { scope, client }),
    }))
}

/// body 缺省或 `scope` 为 null → 本地 scope；字段不全 → 显式错误。
fn parse_scope(body: &[u8]) -> Result<Option<RemoteScope>, String> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Ok(None);
    }
    let value: Value = serde_json::from_slice(body)
        .map_err(|error| format!("capability request body is not JSON: {error}"))?;
    match value.get("scope") {
        None | Some(Value::Null) => Ok(None),
        Some(scope) => RemoteScope::parse(scope).map(Some),
    }
}

/// CSPRNG 令牌（capability 与进程密钥共用）。
pub(crate) fn random_token() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_bodies_are_local_scope() {
        assert!(parse_scope(b"").expect("empty body").is_none());
        assert!(parse_scope(b"  \r\n").expect("blank body").is_none());
        assert!(parse_scope(b"{}").expect("empty object").is_none());
    }

    #[test]
    fn scope_requires_all_three_identities() {
        let error = parse_scope(br#"{"scope":{"profileId":"p"}}"#).expect_err("incomplete scope");
        assert!(error.contains("sourceServerId"), "{error}");
        let scope = parse_scope(
            br#"{"scope":{"sourceServerId":"s","profileId":"p","workspaceIdentity":"w"}}"#,
        )
        .expect("complete scope")
        .expect("scope present");
        assert_eq!(scope.workspace_identity, "w");
    }
}
