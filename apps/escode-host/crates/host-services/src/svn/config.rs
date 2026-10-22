//! svn 常量、子进程环境与二进制候选（TS `svn/config.ts`）。
//!
//! 与 Git 不同，这里不能设置 `LC_ALL=C`：svn 需要把本地路径从系统编码转成 UTF-8，
//! C locale 下遇到非 ASCII 路径会报 E000022。字符集保持 UTF-8，消息固定英文
//! （`LC_MESSAGES=C`），错误归类仍只依赖 `E\d{6}`。

use std::collections::BTreeMap;
use std::time::Duration;

pub const MIN_SUPPORTED_VERSION: &str = "1.8.0";

pub const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(5);
pub const COMMAND_TIMEOUT: Duration = Duration::from_secs(15);
pub const DIFF_TIMEOUT: Duration = Duration::from_secs(20);
pub const REMOTE_READ_TIMEOUT: Duration = Duration::from_secs(30);
pub const LONG_WRITE_TIMEOUT: Duration = Duration::from_secs(10 * 60);
pub const CLEANUP_TIMEOUT: Duration = Duration::from_secs(2 * 60);

pub const OUTPUT_BYTES: usize = 4 * 1024 * 1024;
pub const DIFF_BYTES: usize = 1024 * 1024;

pub const DEFAULT_LOG_LIMIT: usize = 100;
pub const MAX_LOG_LIMIT: usize = 500;
pub const LOG_CACHE_MAX_ENTRIES: usize = 2_000;

/// 统计行数时最多逐个 diff 的文件数；超过后不再统计。
pub const LINE_STAT_MAX_FILES: usize = 200;
/// Blame 最多返回的行数；超出后截断。
pub const MAX_BLAME_LINES: usize = 5_000;
/// 分支图谱最多展示的分支/标签数。
pub const GRAPH_MAX_NODES: usize = 200;
/// 并行探测 copyfrom 的分支数。
pub const GRAPH_COPY_CONCURRENCY: usize = 4;
/// 每个分支回看多少条日志来定位 copyfrom。
pub const GRAPH_COPY_LOG_LIMIT: usize = 500;

const INTERACTIVE_ENV_VARS: [&str; 4] = ["SVN_EDITOR", "SVN_MERGE", "VISUAL", "EDITOR"];

/// TS `getSvnCommandEnv`。
pub fn command_env(process_env: &[(String, String)]) -> BTreeMap<String, String> {
    let mut env: BTreeMap<String, String> = process_env.iter().cloned().collect();
    for name in INTERACTIVE_ENV_VARS {
        env.remove(name);
    }
    // 必然失败的编辑器：即使 svn 漏掉 --non-interactive 也不会挂起等待编辑。
    env.insert(
        "SVN_EDITOR".to_owned(),
        if cfg!(windows) {
            "cmd /c exit 1".to_owned()
        } else {
            "false".to_owned()
        },
    );
    env.insert("TERM".to_owned(), "dumb".to_owned());
    if !cfg!(windows) {
        let current = env
            .get("LC_ALL")
            .or_else(|| env.get("LC_CTYPE"))
            .or_else(|| env.get("LANG"))
            .cloned()
            .unwrap_or_default();
        let lower = current.to_lowercase();
        let fallback = if cfg!(target_os = "macos") {
            "en_US.UTF-8"
        } else {
            "C.UTF-8"
        };
        env.remove("LC_ALL");
        env.insert(
            "LC_CTYPE".to_owned(),
            if lower.contains("utf-8") || lower.contains("utf8") {
                current
            } else {
                fallback.to_owned()
            },
        );
        env.insert("LC_MESSAGES".to_owned(), "C".to_owned());
    }
    env
}

fn env_value(process_env: &[(String, String)], key: &str) -> Option<String> {
    process_env
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.clone())
}

/// TS `getSvnBinaryCandidates`：手动路径 → ESCODE_SVN_BINARY → PATH → Windows TortoiseSVN。
pub fn binary_candidates(
    process_env: &[(String, String)],
    configured: Option<&str>,
) -> Vec<String> {
    let mut candidates: Vec<String> = Vec::new();
    let push = |list: &mut Vec<String>, value: Option<String>| {
        if let Some(value) = value.map(|v| v.trim().to_owned()).filter(|v| !v.is_empty())
            && !list.contains(&value)
        {
            list.push(value);
        }
    };
    push(&mut candidates, configured.map(str::to_owned));
    push(&mut candidates, env_value(process_env, "ESCODE_SVN_BINARY"));
    if !candidates.contains(&"svn".to_owned()) {
        candidates.push("svn".to_owned());
    }
    if cfg!(windows) {
        let program_files = env_value(process_env, "ProgramW6432")
            .or_else(|| env_value(process_env, "ProgramFiles"))
            .unwrap_or_else(|| "C:\\Program Files".to_owned());
        let program_files_x86 = env_value(process_env, "ProgramFiles(x86)")
            .unwrap_or_else(|| "C:\\Program Files (x86)".to_owned());
        for base in [program_files, program_files_x86] {
            let candidate = format!("{}\\TortoiseSVN\\bin\\svn.exe", base.trim_end_matches('\\'));
            if !candidates.contains(&candidate) {
                candidates.push(candidate);
            }
        }
    }
    candidates
}

/// TS `normalizeSvnPath`：统一分隔符并去掉开头的 `./`。
pub fn normalize_svn_path(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    normalized
        .strip_prefix("./")
        .unwrap_or(&normalized)
        .to_owned()
}

/// TS `extractSvnErrorCodes`：只认词边界上的 `E` + 6 位数字。
pub fn extract_error_codes(stderr: &str) -> Vec<String> {
    let bytes = stderr.as_bytes();
    let mut codes = Vec::new();
    let mut index = 0;
    while index + 7 <= bytes.len() {
        if bytes[index] == b'E'
            && bytes[index + 1..index + 7].iter().all(u8::is_ascii_digit)
            && (index == 0 || !is_word_byte(bytes[index - 1]))
            && bytes.get(index + 7).is_none_or(|byte| !is_word_byte(*byte))
        {
            codes.push(stderr[index..index + 7].to_owned());
            index += 7;
            continue;
        }
        index += 1;
    }
    codes
}

fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// TS `compareSvnVersions`：按前三个数字段比较。
pub fn compare_versions(left: &str, right: &str) -> std::cmp::Ordering {
    let parse = |value: &str| -> [u64; 3] {
        let mut parts = [0u64; 3];
        for (index, part) in value.split(['.', '-']).take(3).enumerate() {
            parts[index] = part.parse().unwrap_or(0);
        }
        parts
    };
    parse(left).cmp(&parse(right))
}

/// 解析 `svn --version --quiet` 输出里的版本号前缀（`1.14.3`）。
pub fn parse_version(text: &str) -> Option<String> {
    let trimmed = text.trim();
    let mut end = 0;
    for (index, ch) in trimmed.char_indices() {
        if ch.is_ascii_digit() || ch == '.' {
            end = index + ch.len_utf8();
        } else {
            break;
        }
    }
    let version = trimmed[..end].trim_end_matches('.');
    (!version.is_empty()).then(|| version.to_owned())
}
