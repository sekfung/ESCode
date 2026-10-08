//! 额外 CA（设置页的自定义证书），对齐 TS `adapters/src/network/http-config.ts` 的
//! `resolveTlsCaCertFile`（显式 → `ZCODE_AGENT_CA_CERT`）与 Node 原生信任的 `NODE_EXTRA_CA_CERTS`。
//!
//! 来源与注入方：`packages/services/src/runtime-tools/agentProxyEnv.ts` 在 spawn agent 时同时注入
//! `NODE_EXTRA_CA_CERTS`（Node 启动即信任）与 `ZCODE_AGENT_CA_CERT`（跨运行时/工具子进程）。Rust
//! 没有 Node 的原生行为，所以两个变量都要认，优先级按 TS：显式来源优先。
//! 只返回字节；PEM/DER 的解析交给各 HTTP 客户端（本 crate 不依赖 reqwest）。
use anyhow::{Context, Result};
use std::path::PathBuf;

/// `ZCODE_AGENT_CA_CERT`：显式来源，读不到即失败（TS `readFileSync` 同一条路——证书配错了要让
/// 请求明确失败，而不是静默用系统根证书连出去）。
pub const EXPLICIT_CA_ENV_KEY: &str = "ZCODE_AGENT_CA_CERT";
/// `NODE_EXTRA_CA_CERTS`：Node 的原生来源；文件不存在只警告不失败（对齐 Node 的行为）。
pub const NODE_EXTRA_CA_ENV_KEY: &str = "NODE_EXTRA_CA_CERTS";

/// 应额外信任的根证书字节（0 或 1 份文件；文件里可以有多张证书）。
pub fn extra_ca_certificates(
    scope: crate::net_config::NetworkScope,
) -> Result<Vec<Vec<u8>>> {
    // 显式来源 = `ZCODE_AGENT_CA_CERT` ?? 该作用域的 `network.caCertFile`（TS 合并后的 caCertFile）。
    extra_ca_certificates_from(|key| {
        if key == EXPLICIT_CA_ENV_KEY {
            crate::net_config::explicit_ca_cert_file(scope).map(Into::into)
        } else {
            std::env::var_os(key).filter(|value| !value.is_empty())
        }
    })
}

fn extra_ca_certificates_from(env: impl Fn(&str) -> Option<std::ffi::OsString>) -> Result<Vec<Vec<u8>>> {
    if let Some(path) = env(EXPLICIT_CA_ENV_KEY) {
        let bytes = std::fs::read(PathBuf::from(&path)).with_context(|| {
            format!("Cannot read the configured CA certificate ({EXPLICIT_CA_ENV_KEY})")
        })?;
        return Ok(vec![bytes]);
    }
    Ok(env(NODE_EXTRA_CA_ENV_KEY)
        .and_then(|path| std::fs::read(PathBuf::from(&path)).ok())
        .into_iter()
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_source_wins_and_failures_surface() {
        let dir = tempfile::tempdir().unwrap();
        let explicit = dir.path().join("explicit.pem");
        let fallback = dir.path().join("fallback.pem");
        std::fs::write(&explicit, "explicit").unwrap();
        std::fs::write(&fallback, "fallback").unwrap();
        let env = |key: &str| match key {
            EXPLICIT_CA_ENV_KEY => Some(explicit.clone().into_os_string()),
            NODE_EXTRA_CA_ENV_KEY => Some(fallback.clone().into_os_string()),
            _ => None,
        };
        assert_eq!(
            extra_ca_certificates_from(env).unwrap(),
            vec![b"explicit".to_vec()]
        );
        // 显式来源存在但读不到 → 失败（不静默回落到系统根证书或另一个变量）。
        let missing = dir.path().join("missing.pem");
        let env = |key: &str| (key == EXPLICIT_CA_ENV_KEY).then(|| missing.clone().into_os_string());
        assert!(extra_ca_certificates_from(env).is_err());
    }

    #[test]
    fn node_fallback_is_optional() {
        let dir = tempfile::tempdir().unwrap();
        assert!(extra_ca_certificates_from(|_| None).unwrap().is_empty());
        let missing = dir.path().join("missing.pem");
        let env = |key: &str| (key == NODE_EXTRA_CA_ENV_KEY).then(|| missing.clone().into_os_string());
        assert!(extra_ca_certificates_from(env).unwrap().is_empty());
    }
}
