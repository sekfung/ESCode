//! `safeFlags` 的值域（rust-unified-host.md P1 #11：类型化，未知 kind 不再静默不匹配）。
//! 名字与生成器语料一一对应；`bash_callback_reconciliation` 测试双向对账。
//! 反序列化即校验：未知 kind 在表加载期报错，不会走到运行期的静默不匹配。
use serde::Deserialize;
/// `safeFlags` 的值域（生成器导出的 kind 字符串；P1 #11 类型化）。
/// 未知 kind 在反序列化（表加载）时即报错，不会静默按“不安全”吞掉。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SafeFlagKind {
    /// 无值旗标（`--flag`；带内联值即拒绝）。
    None,
    String,
    Number,
    Char,
    /// 字面量 `{}`。
    Object,
    /// 字面量 `EOF`（heredoc 分隔符）。
    Eof,
}

impl SafeFlagKind {
    pub fn parse(raw: &str) -> Option<Self> {
        Some(match raw {
            "none" => Self::None,
            "string" => Self::String,
            "number" => Self::Number,
            "char" => Self::Char,
            "{}" => Self::Object,
            "EOF" => Self::Eof,
            _ => return None,
        })
    }

    /// 语料里的名字（对账测试与表加载共用）。
    pub fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::String => "string",
            Self::Number => "number",
            Self::Char => "char",
            Self::Object => "{}",
            Self::Eof => "EOF",
        }
    }

    /// 旗标值是否符合该 kind（TS `safeFlagValueMatchesKind`；穷尽 match，无兜底分支）。
    pub fn matches(self, value: &str) -> bool {
        match self {
            Self::None | Self::String => true,
            Self::Number => value.bytes().all(|b| b.is_ascii_digit()),
            // JS length 按 UTF-16 计；单字符在 BMP 内时与 chars 计数一致。
            Self::Char => value.encode_utf16().count() == 1,
            Self::Object => value == "{}",
            Self::Eof => value == "EOF",
        }
    }
}

impl<'de> Deserialize<'de> for SafeFlagKind {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw).ok_or_else(|| {
            serde::de::Error::custom(format!("Unsupported Bash safeFlags kind: {raw}"))
        })
    }
}
