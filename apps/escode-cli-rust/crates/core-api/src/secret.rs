//! 脱敏字符串（rust-unified-host.md P1 #6 的密钥面；语义对标 ZCode-rs `kit::secret::RedactedString`）。
//!
//! 承载 API key、鉴权头等敏感值：`Debug`/`Display` 只输出占位符，`expose()` 是唯一
//! 取值出口（调用点即审计点）。这样日志、错误链、诊断事件里即使无意打印了配置结构，
//! 也不会泄漏凭据（AGENTS.md「不在日志、示例或提交中写入凭据」）。
use std::fmt;

/// 敏感字符串。等值比较按明文（配置比对需要），但任何格式化都是占位符。
#[derive(Clone, Default, PartialEq, Eq)]
pub struct RedactedString(String);

/// 格式化占位符（与对标仓库一致的可读标记）。
pub const REDACTED: &str = "[redacted]";

impl RedactedString {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// 取明文。调用点即审计点：只允许发给 provider 的鉴权头与子进程 env 注入。
    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for RedactedString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(REDACTED)
    }
}

impl fmt::Display for RedactedString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(REDACTED)
    }
}

impl From<String> for RedactedString {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for RedactedString {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formatting_never_reveals_the_secret() {
        let secret = RedactedString::new("sk-live-abc123");
        assert_eq!(format!("{secret:?}"), REDACTED);
        assert_eq!(format!("{secret}"), REDACTED);
        assert_eq!(secret.expose(), "sk-live-abc123");
        // 结构体派生 Debug 同样只看到占位符。
        #[derive(Debug)]
        struct Config {
            #[allow(dead_code)]
            api_key: RedactedString,
        }
        let printed = format!("{:?}", Config { api_key: secret });
        assert!(!printed.contains("sk-live"), "结构打印不得泄漏: {printed}");
        assert!(printed.contains(REDACTED));
    }

    #[test]
    fn equality_uses_the_plaintext() {
        assert_eq!(RedactedString::new("a"), RedactedString::from("a"));
        assert_ne!(RedactedString::new("a"), RedactedString::new("b"));
    }
}
