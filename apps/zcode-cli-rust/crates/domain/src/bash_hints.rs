//! Bash 结果的附加判定（docs/specs/rust-bash-model-content.md），对齐 TS `bash-gh-rate-limit.ts` 与
//! `contracts/model/image-media.ts` 的 `parseImageDataUrl`。纯函数；GitHub 提示的冷却状态由工具层持有。
use regex::Regex;
use std::sync::OnceLock;

pub const GH_RATE_LIMIT_HINT: &str = "<system-reminder>GitHub API rate limit exceeded (5,000/hr shared across all tools and agents). Run `gh api rate_limit --jq .resources` and sleep until reset before further gh calls. If polling in a loop, use ScheduleWakeup instead of retrying.</system-reminder>";
pub const GH_RATE_LIMIT_COOLDOWN_MS: u64 = 60_000;
const GH_EXCLUDED: [&str; 6] = ["auth", "help", "version", "alias", "completion", "config"];

/// TS `GH_COMMAND_RE` 与 `GH_RATE_LIMIT_RE`：命令中有非管理类 `gh` 子命令，且输出含限流文案。
/// Rust 正则没有负向先行断言，改为取 `gh` 后的词单独比较（与 `(?!auth\b|…)` 等价）。
pub fn gh_rate_limited(command: &str, output: &str) -> bool {
    static COMMAND: OnceLock<Regex> = OnceLock::new();
    static LIMIT: OnceLock<Regex> = OnceLock::new();
    let command_re = COMMAND.get_or_init(|| Regex::new(r"(?:^|[;&|]|\b(?:then|do)\b)\s*gh\s+").unwrap());
    let limit_re = LIMIT.get_or_init(|| {
        Regex::new(r"(?i)API rate limit (?:already )?exceeded|exceeded a secondary rate limit|\bRATE_LIMITED\b").unwrap()
    });
    let gh = command_re.find_iter(command).any(|m| {
        let rest = &command[m.end()..];
        !GH_EXCLUDED.iter().any(|word| {
            rest.strip_prefix(word)
                .is_some_and(|after| after.chars().next().is_none_or(|c| !(c.is_alphanumeric() || c == '_')))
        })
    });
    gh && limit_re.is_match(output)
}

/// TS `parseImageDataUrl`（不允许空白）：返回（规范化的媒体类型, base64）。
pub fn image_data_url(value: &str) -> Option<(String, String)> {
    let rest = value.trim().strip_prefix("data:")?;
    let (media, payload) = rest.split_once(";base64,")?;
    let valid_media = media.len() > "image/".len()
        && media.get(..6).is_some_and(|p| p.eq_ignore_ascii_case("image/"))
        && media.get(6..).is_some_and(|rest| rest.chars().all(|c| c.is_ascii_alphanumeric() || ".+_-".contains(c)));
    if !valid_media || payload.is_empty() || payload.chars().any(char::is_whitespace) {
        return None;
    }
    // TS isValidBase64Payload：`length % 4 !== 1` 且 `/^[A-Za-z0-9+/]+={0,2}$/`。
    let body = payload.trim_end_matches('=');
    let valid = payload.len() % 4 != 1
        && !body.is_empty()
        && payload.len() - body.len() <= 2
        && body.chars().all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/');
    if !valid {
        return None;
    }
    let media = media.to_ascii_lowercase();
    let media = if media == "image/jpg" { "image/jpeg".to_owned() } else { media };
    Some((media, payload.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gh_hint_requires_real_gh_call_and_limit_text() {
        assert!(gh_rate_limited("gh api repos/x/y", "API rate limit exceeded for user"));
        assert!(gh_rate_limited("cd x && gh pr list", "API rate limit already exceeded"));
        assert!(gh_rate_limited("if true; then gh issue list; fi", "RATE_LIMITED"));
        assert!(!gh_rate_limited("gh auth status", "API rate limit exceeded"));
        assert!(!gh_rate_limited("echo gh api", "API rate limit exceeded"));
        assert!(!gh_rate_limited("gh api x", "all good"));
        assert!(gh_rate_limited("gh authx", "exceeded a secondary rate limit"));
    }

    #[test]
    fn parses_strict_image_data_urls() {
        assert_eq!(image_data_url(" data:image/JPG;base64,AAAA ").unwrap().0, "image/jpeg");
        assert!(image_data_url("data:image/png;base64,AA AA").is_none());
        assert!(image_data_url("data:text/plain;base64,AAAA").is_none());
        assert!(image_data_url("data:image/png;base64,AAAAA").is_none());
        assert!(image_data_url("data:image/png;base64,AAA").is_some());
        assert!(image_data_url("hello").is_none());
    }
}
