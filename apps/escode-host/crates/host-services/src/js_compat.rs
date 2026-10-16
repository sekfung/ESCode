//! 与 JS 语义对齐的小工具（Rust 标准库与 JS 在这些点上不一致）。

/// JS `String.prototype.trim` 的空白集（WhiteSpace + LineTerminator）。不能用 Rust 的 `str::trim`：
/// 它按 Unicode `White_Space`，含 U+0085、不含 U+FEFF，与 JS 恰好相反。
pub fn is_js_space(c: char) -> bool {
    matches!(
        c,
        '\u{9}'..='\u{d}'
            | ' '
            | '\u{a0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200a}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202f}'
            | '\u{205f}'
            | '\u{3000}'
            | '\u{feff}'
    )
}

/// JS `trim()`。
pub fn js_trim(text: &str) -> &str {
    text.trim_matches(is_js_space)
}
