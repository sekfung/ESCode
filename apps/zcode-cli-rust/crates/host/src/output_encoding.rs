//! 工具输出的 Windows 旧代码页解码（docs/specs/rust-windows-output-encoding.md），对齐 TS
//! `adapters/src/exec/outputEncoding.ts`。修复：Rust 原先一律按 UTF-8 lossy 解码，中文 Windows 上 cmd / 旧版工具
//! 按 OEM 代码页（936 等）输出的文本对模型与 UI 都是乱码。

/// 覆盖旧编码的环境变量（TS `ZCODE_WINDOWS_OUTPUT_ENCODING`）。
pub const OVERRIDE_ENV_KEY: &str = "ZCODE_WINDOWS_OUTPUT_ENCODING";
#[cfg_attr(not(windows), allow(dead_code))] // 只有 Windows 的旧编码解析用到；单测在各平台都覆盖。
const UTF8_CODE_PAGE: u32 = 65001;
#[cfg_attr(not(windows), allow(dead_code))] // 只有 Windows 的旧编码解析用到；单测在各平台都覆盖。
const GB18030_CODE_PAGE: u32 = 54936;

/// 整段解码工具输出：合法 UTF-8 原样；真正非法的 UTF-8 在有旧编码时按旧编码解码；其余 UTF-8 lossy。
/// 与 TS 的偏离：末尾仅被截断的多字节序列不切换旧编码（TS 会整段改用旧编码，见 spec 已知差异）。
pub fn decode_output(bytes: &[u8]) -> String {
    decode_with(bytes, legacy_code_page())
}

fn decode_with(bytes: &[u8], legacy: Option<u32>) -> String {
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_owned(),
        Err(error) if error.error_len().is_none() => String::from_utf8_lossy(bytes).into_owned(),
        Err(_) => legacy
            .and_then(|code_page| decode_code_page(bytes, code_page))
            .unwrap_or_else(|| String::from_utf8_lossy(bytes).into_owned()),
    }
}

/// `ZCODE_WINDOWS_OUTPUT_ENCODING` 的取值 → 代码页。识别 `cpNNN` / `NNN` 与常见别名；不可识别为 `None`
/// （TS `iconv.encodingExists` 失败即无旧编码）。
#[cfg_attr(not(windows), allow(dead_code))] // 只有 Windows 的旧编码解析用到；单测在各平台都覆盖。
fn override_code_page(value: &str) -> Option<u32> {
    let value = value.trim().to_ascii_lowercase();
    let digits = value
        .strip_prefix("cp")
        .or_else(|| value.strip_prefix("windows-"))
        .unwrap_or(&value);
    if let Ok(code_page) = digits.parse::<u32>() {
        return Some(code_page);
    }
    Some(match value.as_str() {
        "utf8" | "utf-8" => UTF8_CODE_PAGE,
        "gbk" | "gb2312" => 936,
        "gb18030" => GB18030_CODE_PAGE,
        "big5" => 950,
        "shift_jis" | "shift-jis" | "sjis" => 932,
        "euc-kr" | "euckr" => 949,
        "ibm866" => 866,
        _ => return None,
    })
}

/// TS `resolveWindowsLocaleLegacyEncoding`：locale 文本小写后按子串推断。
#[cfg_attr(not(windows), allow(dead_code))] // 只有 Windows 的旧编码解析用到；单测在各平台都覆盖。
fn locale_code_page(locale_text: &str) -> u32 {
    let text = locale_text.to_lowercase();
    let any = |needles: &[&str]| needles.iter().any(|needle| text.contains(needle));
    if any(&["zh", "chinese", "cn", "hans", "hant"]) {
        GB18030_CODE_PAGE
    } else if any(&["ja", "japanese", "jp"]) {
        932
    } else if any(&["ko", "korean", "kr"]) {
        949
    } else if any(&["ru", "russian"]) {
        866
    } else {
        437
    }
}

#[cfg(not(windows))]
fn legacy_code_page() -> Option<u32> {
    None
}

#[cfg(not(windows))]
fn decode_code_page(_bytes: &[u8], _code_page: u32) -> Option<String> {
    None
}

/// 进程内解析一次（TS 每次执行跑 `chcp`；代码页与覆盖变量在进程生命周期内不变）。
#[cfg(windows)]
fn legacy_code_page() -> Option<u32> {
    static CODE_PAGE: std::sync::OnceLock<Option<u32>> = std::sync::OnceLock::new();
    *CODE_PAGE.get_or_init(resolve_legacy_code_page)
}

#[cfg(windows)]
fn resolve_legacy_code_page() -> Option<u32> {
    use windows_sys::Win32::Globalization::{GetOEMCP, IsValidCodePage};
    if let Some(value) = std::env::var(OVERRIDE_ENV_KEY)
        .ok()
        .filter(|value| !value.trim().is_empty())
    {
        return override_code_page(&value)
            .filter(|&code_page| unsafe { IsValidCodePage(code_page) } != 0);
    }
    // TS 在新开的 cmd 里跑 `chcp`：无控制台的子进程拿到的就是 OEM 代码页。
    let active = unsafe { GetOEMCP() };
    if active != UTF8_CODE_PAGE && unsafe { IsValidCodePage(active) } != 0 {
        return Some(active);
    }
    let locale_text = ["LC_ALL", "LC_CTYPE", "LANG"]
        .iter()
        .filter_map(|key| std::env::var(key).ok())
        .chain(user_default_locale_name())
        .collect::<Vec<_>>()
        .join(" ");
    Some(locale_code_page(&locale_text))
}

#[cfg(windows)]
fn user_default_locale_name() -> Option<String> {
    use windows_sys::Win32::Globalization::GetUserDefaultLocaleName;
    let mut buffer = [0u16; 85];
    let length = unsafe { GetUserDefaultLocaleName(buffer.as_mut_ptr(), buffer.len() as i32) };
    (length > 1).then(|| String::from_utf16_lossy(&buffer[..length as usize - 1]))
}

#[cfg(windows)]
fn decode_code_page(bytes: &[u8], code_page: u32) -> Option<String> {
    use windows_sys::Win32::Globalization::MultiByteToWideChar;
    if bytes.is_empty() {
        return Some(String::new());
    }
    let length = i32::try_from(bytes.len()).ok()?;
    let needed = unsafe {
        MultiByteToWideChar(code_page, 0, bytes.as_ptr(), length, std::ptr::null_mut(), 0)
    };
    if needed <= 0 {
        return None;
    }
    let mut wide = vec![0u16; needed as usize];
    let written = unsafe {
        MultiByteToWideChar(code_page, 0, bytes.as_ptr(), length, wide.as_mut_ptr(), needed)
    };
    (written > 0).then(|| String::from_utf16_lossy(&wide[..written as usize]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_is_kept_and_truncated_tail_does_not_switch_to_legacy() {
        assert_eq!(decode_with("版本 ok".as_bytes(), Some(936)), "版本 ok");
        let mut cut = "版本".as_bytes().to_vec();
        cut.pop();
        assert_eq!(decode_with(&cut, Some(936)), "版\u{fffd}");
    }

    #[test]
    fn invalid_utf8_without_legacy_is_lossy() {
        assert_eq!(decode_with(&[0xb0, 0xe6, b'x'], None), "\u{fffd}\u{fffd}x");
    }

    #[cfg(windows)]
    #[test]
    fn legacy_code_page_decodes_gbk() {
        // 「版本」的 GBK 字节（cmd `ver` 在中文 Windows 上的输出片段）。
        assert_eq!(decode_with(&[0xb0, 0xe6, 0xb1, 0xbe], Some(936)), "版本");
        assert_eq!(decode_with(&[0xb0, 0xe6, 0xb1, 0xbe], Some(54936)), "版本");
    }

    #[test]
    fn override_values() {
        assert_eq!(override_code_page("cp936"), Some(936));
        assert_eq!(override_code_page(" 936 "), Some(936));
        assert_eq!(override_code_page("GBK"), Some(936));
        assert_eq!(override_code_page("utf-8"), Some(UTF8_CODE_PAGE));
        assert_eq!(override_code_page("windows-1252"), Some(1252));
        assert_eq!(override_code_page("klingon"), None);
    }

    #[test]
    fn locale_inference_matches_ts_table() {
        assert_eq!(locale_code_page("zh-CN"), GB18030_CODE_PAGE);
        assert_eq!(locale_code_page("ja-JP"), 932);
        assert_eq!(locale_code_page("ko-KR"), 949);
        assert_eq!(locale_code_page("ru-RU"), 866);
        assert_eq!(locale_code_page("en-US"), 437);
        // 子串匹配与 TS 正则一致：`en_US.UTF-8 zh-TW` 命中 zh。
        assert_eq!(locale_code_page("en_US.UTF-8 zh-TW"), GB18030_CODE_PAGE);
    }
}
