//! HttpModel 的客户端与计时辅助：自定义 CA、首包 / 空闲截止时间。

use crate::contract::ModelFailure;
use std::time::Duration;
use tokio::time::Instant;

type Result<T> = std::result::Result<T, ModelFailure>;

/// 把设置页注入的 CA 加进客户端根证书（PEM 可含多张；失败再按 DER 试一次）。
/// 读文件失败（`ZCODE_AGENT_CA_CERT` 配错）即让本次请求以配置错误收口，与 TS `readFileSync` 同路。
pub(super) fn add_extra_ca_certificates(
    mut builder: reqwest::ClientBuilder,
) -> Result<reqwest::ClientBuilder> {
    let misplaced = || ModelFailure::new("invalid_request", false);
    for bytes in zcode_cli_host::tls_ca::extra_ca_certificates(zcode_cli_host::net_config::NetworkScope::Process).map_err(|_| misplaced())? {
        let certificates = match reqwest::Certificate::from_pem_bundle(&bytes) {
            Ok(certificates) => certificates,
            Err(_) => vec![reqwest::Certificate::from_der(&bytes).map_err(|_| misplaced())?],
        };
        for certificate in certificates {
            builder = builder.add_root_certificate(certificate);
        }
    }
    Ok(builder)
}

pub(super) fn after(ms: u64) -> Option<Instant> {
    if ms == 0 {
        None
    } else {
        Instant::now().checked_add(Duration::from_millis(ms))
    }
}

pub(super) async fn deadline(at: Option<Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}
