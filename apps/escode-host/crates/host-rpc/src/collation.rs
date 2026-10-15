//! Node 口径的字符串排序（`String.prototype.localeCompare`，ICU 默认 collation）。
//!
//! `file.readdir` 的第二排序键是 `a.name.localeCompare(b.name)`：无参数时用**默认 locale**
//! （V8/ICU 从环境解析）与默认选项（`sensitivity: "variant"`、`numeric: false`、
//! `ignorePunctuation: false`）。默认 locale 因此是环境相关的事实，选择器要按同一套规则解析，
//! 否则同一目录在两端的顺序会不一致（差分证据见 `tests/` 的 readdir 对账用例）。
//!
//! 这里用 ICU4X（`icu_collator`，UCA/CLDR 数据）复现该顺序，locale 解析规则按 Node 实测：
//! `LC_ALL` > `LC_MESSAGES` > `LANG` > `en-US`；`C`/`POSIX`（含 `.UTF-8` 后缀）归 `en-US`；
//! `zh_CN.UTF-8` → `zh-CN`（去编码后缀、`_` → `-`）。

use std::sync::OnceLock;

use icu_collator::options::CollatorOptions;
use icu_collator::{CollatorBorrowed, CollatorPreferences};
use icu_locale_core::Locale;

/// 默认 locale 的候选环境变量，按 Node（ICU `uloc_getDefault`）优先级。
const LOCALE_ENV_ORDER: [&str; 3] = ["LC_ALL", "LC_MESSAGES", "LANG"];

/// 解析默认 locale：`LC_ALL` > `LC_MESSAGES` > `LANG` > `en-US`。
/// `lookup` 是环境读取口（生产走 `core_api::env`；测试注入固定表，不依赖真实进程 env）。
pub(crate) fn default_locale(lookup: impl Fn(&str) -> Option<String>) -> String {
    for name in LOCALE_ENV_ORDER {
        if let Some(raw) = lookup(name)
            && let Some(locale) = normalize_locale(&raw)
        {
            return locale;
        }
    }
    "en-US".to_owned()
}

/// 归一化 POSIX locale：`zh_CN.UTF-8` → `zh-CN`；`C`/`POSIX`（含编码后缀）→ 无（归默认）。
fn normalize_locale(raw: &str) -> Option<String> {
    let value = raw.trim();
    if value.is_empty() {
        return None;
    }
    // 去编码/修饰后缀（`.UTF-8`、`@euro`）。两步各自回退到上一步的结果。
    let without_encoding = value.split_once('.').map_or(value, |(head, _)| head);
    let value = without_encoding
        .split_once('@')
        .map_or(without_encoding, |(head, _)| head);
    if value.is_empty() || value.eq_ignore_ascii_case("C") || value.eq_ignore_ascii_case("POSIX") {
        return None;
    }
    let locale = value.replace('_', "-");
    // 非法标签（拼写错误）交回默认：ICU 也无此 locale 数据。
    locale.parse::<Locale>().ok()?;
    Some(locale)
}

/// 进程默认排序器（首次调用构建；默认 locale 在进程内不再变）。
/// 用 `CollatorBorrowed<'static>`：只持有编译期数据的 `&'static` 引用，
/// 因此可放进 `OnceLock` 跨线程共享（`Collator` 持有 `Rc`，不是 `Sync`）。
fn collator() -> &'static CollatorBorrowed<'static> {
    static COLLATOR: OnceLock<CollatorBorrowed<'static>> = OnceLock::new();
    COLLATOR.get_or_init(|| {
        let locale = default_locale(|name| escode_cli_core_api::env::var(name).ok());
        build(&locale)
    })
}

/// 按 locale 建排序器；无该 locale 数据时退回根 collation（与 ICU 的回退一致）。
fn build(locale: &str) -> CollatorBorrowed<'static> {
    let locale = locale.parse::<Locale>().unwrap_or(Locale::UNKNOWN);
    CollatorBorrowed::try_new(
        CollatorPreferences::from(&locale),
        CollatorOptions::default(),
    )
    .unwrap_or_else(|_| {
        CollatorBorrowed::try_new(
            CollatorPreferences::from(&Locale::UNKNOWN),
            CollatorOptions::default(),
        )
        .expect("root collation builds")
    })
}

/// `localeCompare(x, "en")`：显式 `en` locale，不随环境变化（Project Memory 目录排序用）。
pub(crate) fn compare_en(left: &str, right: &str) -> std::cmp::Ordering {
    static EN: OnceLock<CollatorBorrowed<'static>> = OnceLock::new();
    EN.get_or_init(|| build("en")).compare(left, right)
}

/// 与 `localeCompare` 同义的比较（供 `sort_by`；稳定排序保留 readdir 的原始顺序作为末位键）。
pub(crate) fn compare_names(left: &str, right: &str) -> std::cmp::Ordering {
    collator().compare(left, right)
}

#[cfg(test)]
#[path = "collation_tests.rs"]
mod tests;
