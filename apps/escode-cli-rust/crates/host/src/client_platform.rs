//! 模型请求客户端头用到的平台事实（docs/specs/rust-model-request-headers.md），对齐 Node `process.platform`、
//! `os.arch()`、`os.release()` 与 `Intl.DateTimeFormat().resolvedOptions()`。进程内解析一次。
use std::sync::OnceLock;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClientPlatform {
    /// Node `process.platform`：`win32` / `darwin` / `linux` …
    pub platform: &'static str,
    /// Node `os.arch()`：`x64` / `arm64` / `ia32` …
    pub arch: &'static str,
    /// `windows` / `macos` / `linux`（Node normalizeOsCategory）。
    pub os_category: &'static str,
    /// Node `os.release()`。
    pub os_release: Option<String>,
    /// 进程默认 locale（BCP 47）。
    pub locale: Option<String>,
    /// 本机 IANA 时区。
    pub timezone: Option<String>,
}

pub fn current() -> &'static ClientPlatform {
    static PLATFORM: OnceLock<ClientPlatform> = OnceLock::new();
    PLATFORM.get_or_init(|| {
        let platform = node_platform(std::env::consts::OS);
        ClientPlatform {
            platform,
            arch: node_arch(std::env::consts::ARCH),
            os_category: os_category(platform),
            os_release: os_release(),
            locale: locale(),
            timezone: iana_time_zone::get_timezone().ok().map(|tz| canonical_timezone(&tz)),
        }
    })
}

/// V8 `Intl.DateTimeFormat().resolvedOptions().timeZone` 把 UTC 的各种别名归一为 `UTC`。修复：CI runner 的系统时区为
/// `Etc/UTC`，Rust 原样发出，Node 发 `UTC`。其余 ICU 旧链接名（如 `US/Pacific`）的改写未对齐（系统时区通常已是规范名）。
fn canonical_timezone(tz: &str) -> String {
    const UTC_ALIASES: [&str; 18] = [
        "UTC", "Etc/UTC", "Etc/GMT", "GMT", "Etc/UCT", "UCT", "Etc/Universal", "Universal", "Etc/Zulu",
        "Zulu", "Etc/Greenwich", "Greenwich", "Etc/GMT0", "GMT0", "Etc/GMT+0", "Etc/GMT-0", "GMT+0", "GMT-0",
    ];
    if UTC_ALIASES.iter().any(|alias| alias.eq_ignore_ascii_case(tz)) {
        "UTC".to_owned()
    } else {
        tz.to_owned()
    }
}

fn node_platform(os: &'static str) -> &'static str {
    match os {
        "windows" => "win32",
        "macos" => "darwin",
        other => other,
    }
}

fn node_arch(arch: &'static str) -> &'static str {
    match arch {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        "x86" => "ia32",
        "powerpc64" => "ppc64",
        other => other,
    }
}

fn os_category(platform: &str) -> &'static str {
    match platform {
        "darwin" => "macos",
        "win32" => "windows",
        _ => "linux",
    }
}

/// POSIX locale 名（`zh_CN.UTF-8`、`en_US@euro`）→ BCP 47（`zh-CN`）；`C` / `POSIX` / 空 → `en-US`（ICU 的回落）。
#[cfg_attr(windows, allow(dead_code))] // 只有 POSIX 的 locale 解析用到；单测在各平台都覆盖。
fn bcp47(posix: &str) -> String {
    let base = posix.split(['.', '@']).next().unwrap_or("").trim();
    if base.is_empty() || base == "C" || base == "POSIX" {
        return "en-US".into();
    }
    base.replace('_', "-")
}

#[cfg(windows)]
fn locale() -> Option<String> {
    crate::output_encoding::user_default_locale_name()
}

#[cfg(not(windows))]
fn locale() -> Option<String> {
    // ICU 读取顺序：LC_ALL → LC_MESSAGES → LANG。
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .find_map(|key| std::env::var(key).ok().filter(|v| !v.trim().is_empty()))
        .map(|value| bcp47(&value))
        .or_else(|| Some("en-US".into()))
}

#[cfg(windows)]
fn os_release() -> Option<String> {
    // Node os.release() 在 Windows 上走 RtlGetVersion（不受兼容性清单影响）：`major.minor.build`。
    #[repr(C)]
    struct OsVersionInfoW {
        size: u32,
        major: u32,
        minor: u32,
        build: u32,
        platform_id: u32,
        csd: [u16; 128],
    }
    #[link(name = "ntdll")]
    unsafe extern "system" {
        fn RtlGetVersion(info: *mut OsVersionInfoW) -> i32;
    }
    let mut info = OsVersionInfoW {
        size: std::mem::size_of::<OsVersionInfoW>() as u32,
        major: 0,
        minor: 0,
        build: 0,
        platform_id: 0,
        csd: [0; 128],
    };
    (unsafe { RtlGetVersion(&mut info) } == 0)
        .then(|| format!("{}.{}.{}", info.major, info.minor, info.build))
}

#[cfg(not(windows))]
fn os_release() -> Option<String> {
    let mut name: libc::utsname = unsafe { std::mem::zeroed() };
    if unsafe { libc::uname(&mut name) } != 0 {
        return None;
    }
    let release = unsafe { std::ffi::CStr::from_ptr(name.release.as_ptr()) };
    Some(release.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_to_node_names() {
        assert_eq!(node_platform("windows"), "win32");
        assert_eq!(node_platform("macos"), "darwin");
        assert_eq!(node_platform("linux"), "linux");
        assert_eq!(node_arch("x86_64"), "x64");
        assert_eq!(node_arch("aarch64"), "arm64");
        assert_eq!(os_category("win32"), "windows");
        assert_eq!(os_category("darwin"), "macos");
        assert_eq!(os_category("freebsd"), "linux");
    }

    #[test]
    fn utc_aliases_canonicalize_like_v8() {
        for tz in ["Etc/UTC", "GMT", "Etc/Zulu", "UCT", "Etc/GMT+0", "utc"] {
            assert_eq!(canonical_timezone(tz), "UTC", "{tz}");
        }
        assert_eq!(canonical_timezone("Asia/Shanghai"), "Asia/Shanghai");
        assert_eq!(canonical_timezone("Etc/GMT+8"), "Etc/GMT+8");
    }

    #[test]
    fn posix_locale_to_bcp47() {
        assert_eq!(bcp47("zh_CN.UTF-8"), "zh-CN");
        assert_eq!(bcp47("en_US@euro"), "en-US");
        assert_eq!(bcp47("C"), "en-US");
        assert_eq!(bcp47("POSIX"), "en-US");
        assert_eq!(bcp47("ja"), "ja");
    }

    #[test]
    fn current_platform_is_populated() {
        let p = current();
        assert!(!p.platform.is_empty() && !p.arch.is_empty());
        assert!(p.os_release.as_deref().is_some_and(|r| !r.is_empty()));
    }
}
