//! 插件安装/市场失败的**类型化分类单点**（rust-unified-host.md P1 #9「错误 enum 化替代文案前缀匹配」）。
//!
//! 之前 `plugin_install::install_diagnostic` 与 `plugin_market_write::failure_code` 各有一份
//! 按错误**文案**（`starts_with`/`contains`）分类的实现：文案改一个字就静默落到兜底 code，
//! 且两份实现会各自漂移。这里：
//!
//! 1. 失败在**产生点**用 [`PluginFailure`] 抛出（`Display` 就是原来的文案，线上 `message` 不变）；
//! 2. 分类只做 downcast（先 `PluginFailure`，其次已有的 `plugin_git::SourceError`）；
//! 3. `code` 字符串与 TS `toMarketplaceInstallDiagnostic` / `toValidationDiagnostic` 一一对应。

use std::fmt;

/// 插件失败的分类（`code()` 即线上诊断的 `code` 字段）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    NotFound,
    DependencyCrossMarketplace,
    DependencyCycle,
    DependencyMissing,
    UnsupportedSource,
    /// 兜底：未分类的失败（TS `plugin_marketplace_invalid`）。
    Invalid,
}

impl Kind {
    pub(super) fn code(self) -> &'static str {
        match self {
            Self::NotFound => "plugin_not_found",
            Self::DependencyCrossMarketplace => "plugin_dependency_cross_marketplace",
            Self::DependencyCycle => "plugin_dependency_cycle",
            Self::DependencyMissing => "plugin_dependency_missing",
            Self::UnsupportedSource => "plugin_marketplace_source_unsupported",
            Self::Invalid => "plugin_marketplace_invalid",
        }
    }
}

/// 带分类的插件失败：`Display` 是用户可见文案（与 TS 逐字一致），分类只走 downcast。
#[derive(Debug)]
pub(super) struct PluginFailure {
    kind: Kind,
    message: String,
}

impl PluginFailure {
    pub(super) fn new(kind: Kind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub(super) fn kind(&self) -> Kind {
        self.kind
    }
}

impl fmt::Display for PluginFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for PluginFailure {}

/// 抛一个已分类的插件失败。
macro_rules! plugin_failure {
    ($kind:expr, $($arg:tt)*) => {
        anyhow::Error::new($crate::plugin_failure::PluginFailure::new($kind, format!($($arg)*)))
    };
}
pub(super) use plugin_failure;

/// 失败分类单点：两个诊断出口（安装诊断 / 市场刷新诊断）共用。
pub(super) fn classify(error: &anyhow::Error) -> &'static str {
    if let Some(failure) = error.downcast_ref::<PluginFailure>() {
        return failure.kind().code();
    }
    if let Some(source) = error.downcast_ref::<super::plugin_git::SourceError>() {
        return source.code;
    }
    Kind::Invalid.code()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classified_failures_keep_the_ts_codes() {
        let failure = anyhow::Error::new(PluginFailure::new(
            Kind::DependencyCycle,
            "Plugin dependency cycle: a -> b",
        ));
        assert_eq!(classify(&failure), "plugin_dependency_cycle");
        assert_eq!(failure.to_string(), "Plugin dependency cycle: a -> b");
        let wrapped = failure.context("install failed");
        assert_eq!(
            classify(&wrapped),
            "plugin_dependency_cycle",
            "context 不影响分类"
        );
        assert_eq!(
            classify(&anyhow::anyhow!("anything else")),
            "plugin_marketplace_invalid"
        );
        assert_eq!(Kind::NotFound.code(), "plugin_not_found");
    }
}
