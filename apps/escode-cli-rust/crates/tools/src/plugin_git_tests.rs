//! plugin_git.rs 的单测（从内联 `mod tests` 挪出，保持文件在 400 行以内）。
use super::*;
use anyhow::anyhow;

#[test]
fn github_urls_and_redaction() {
    assert_eq!(
        public_github("https://github.com/acme/tool.git"),
        Some(("acme".into(), "tool".into()))
    );
    assert_eq!(
        public_github("https://github.com/acme/tool/tree/main"),
        None
    );
    assert_eq!(public_github("https://user:pw@github.com/acme/tool"), None);
    assert_eq!(public_github("file:///repo"), None);
    assert_eq!(
        redact_source("https://user:secret@example.com/r.git"),
        "https://example.com/r.git"
    );
    // WHATWG URL 把 `user:secret@host:repo` 解析成 scheme=user 的合法 URL（TS 同样原样返回）；
    // 只有解析失败的 `a:b@c` 形式才整体替换。
    assert_eq!(
        redact_source("user:secret@host:repo"),
        "user:secret@host:repo"
    );
    assert_eq!(
        redact_source("git@github.com:a/b.git"),
        "git@github.com:a/b.git"
    );
    assert!(retryable(&anyhow!("fatal: early EOF")));
    assert!(!retryable(&anyhow!("repository not found")));
}
