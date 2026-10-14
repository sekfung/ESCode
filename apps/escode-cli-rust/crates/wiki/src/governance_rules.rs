//! governance 的纯规则判定（从 governance.rs 拆出）：密钥形态筛查与提案相似度。

use escode_cli_domain::wiki::text::tokenize;
use std::collections::BTreeSet;

/// 明显的密钥/凭据形态：提案里出现即拒绝（防止敏感信息沉淀到 Wiki，尤其是 Global Wiki）。
pub(super) fn looks_secret(text: &str) -> bool {
    let lower = text.to_lowercase();
    if lower.contains("-----begin") && lower.contains("private key") {
        return true;
    }
    let akia = text.as_bytes().windows(20).any(|w| {
        w.starts_with(b"AKIA")
            && w[4..]
                .iter()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
    });
    let assignment = [
        "api_key",
        "apikey",
        "api-key",
        "secret",
        "password",
        "passwd",
        "access_token",
        "auth_token",
    ]
    .iter()
    .any(|key| {
        lower.match_indices(key).any(|(i, _)| {
            let rest = lower[i + key.len()..].trim_start();
            let value = rest
                .strip_prefix(['=', ':'])
                .map(str::trim_start)
                .unwrap_or("");
            let value: String = value
                .chars()
                .take_while(|c| !c.is_whitespace() && *c != ',')
                .collect();
            value.trim_matches(['"', '\'']).len() >= 12
        })
    });
    akia || assignment
}

/// 提案正文去掉追加的 Rationale / Evidence 段，只比较规则本身。
pub(super) fn core_body(body: &str) -> &str {
    body.split("\n\n## Rationale").next().unwrap_or(body)
}

pub(super) fn similarity(a: &str, b: &str) -> f64 {
    let a: BTreeSet<String> = tokenize(a).into_iter().collect();
    let b: BTreeSet<String> = tokenize(b).into_iter().collect();
    let union = a.union(&b).count();
    if union == 0 {
        0.0
    } else {
        a.intersection(&b).count() as f64 / union as f64
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn secret_screening() {
        assert!(super::looks_secret("api_key = sk_live_abcdefghijklmnop"));
        assert!(super::looks_secret("AKIAABCDEFGHIJKLMNOP"));
        assert!(!super::looks_secret(
            "Never log the api_key value; read it from the vault."
        ));
    }
}
