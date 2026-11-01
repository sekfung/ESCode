//! Provider 模型规则匹配器：TS `new RegExp("^(?:pattern)$", flags)` 的 Rust 对等实现。
//!
//! 历史 Built-in 发布过 `.*glm-4\.6(?!v).*` 这类带前瞻的 `modelMatch`。`regex` crate 不支持
//! 前瞻/后顾/反向引用：Host 侧会把整条规则丢弃（`rule pattern unsupported` 后永不命中），
//! CLI 侧会让整份 Registry 解析失败，两者都与 TS 不一致。`fancy-regex` 对不需要回溯的写法
//! 仍交给 `regex`，只有前瞻/后顾/反向引用才走回溯引擎，因此普通规则的匹配语义不变。

use fancy_regex::Regex;

/// 一条规则字段（`modelMatch` / `apiTypeMatch` / `baseUrlMatch`）的匹配器。
#[derive(Clone, Debug)]
pub struct RuleMatcher {
    regex: Regex,
}

impl RuleMatcher {
    /// 编译为 TS 同款的整串匹配（`^(?:pattern)$`），`ignore_case` 对应 JS 的 `i` 标志。
    pub fn new(pattern: &str, ignore_case: bool) -> Result<Self, fancy_regex::Error> {
        let regex = fancy_regex::RegexBuilder::new(&format!("^(?:{pattern})$"))
            .case_insensitive(ignore_case)
            .build()?;
        Ok(Self { regex })
    }

    /// 回溯预算耗尽（`fancy-regex` 的运行时错误）按“不命中”处理：单条规则的匹配失败不能
    /// 中断整份 Registry 解析，也不能悄悄放行规则。TS 没有这个上限，因此只影响病态正则。
    pub fn is_match(&self, text: &str) -> bool {
        self.regex.is_match(text).unwrap_or(false)
    }
}
