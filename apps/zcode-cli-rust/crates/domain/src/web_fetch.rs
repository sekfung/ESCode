//! WebFetch 纯逻辑，逐条对齐 TS `webfetch-url.ts`、`webfetch-egress-guard.ts`、`webfetch-content.ts`、
//! `webfetch-processing.ts`（语料见 tests/fixtures/webfetch_corpus.json）。见 docs/specs/rust-webfetch.md。

use std::sync::LazyLock;

use regex::Regex;
use url::Url;

use crate::web_fetch_ip::{is_public, parse_ip};

pub const MAX_URL_CHARS: usize = 2_000;
pub const MAX_MODEL_INPUT_CHARS: usize = 100_000;
pub const MAX_REDIRECTS: usize = 10;
pub const MAX_RESPONSE_BYTES: usize = 10 * 1024 * 1024;
pub const TIMEOUT_MS: u64 = 60_000;
pub const USER_AGENT: &str = "ZCode-WebFetch/0.1 (+https://zcode.ai; coding-agent-cli)";
pub const ACCEPT: &str = "text/markdown, text/html, */*";
pub const MAX_PROCESSING_OUTPUT_TOKENS: usize = 4_096;
pub const EMPTY_RESULT: &str = "WebFetch completed, but the extraction model returned no text.";
const TRUNCATION_SUFFIX: &str = "\n\n[WebFetch content truncated before prompt processing]";

/// TS `normalizeWebFetchUrl`：长度、解析、协议、凭据、http→https、主机形态。错误为 TS 文案。
pub fn normalize_url(value: &str) -> Result<Url, String> {
    if value.encode_utf16().count() > MAX_URL_CHARS {
        return Err("URL is too long".into());
    }
    let mut url = Url::parse(value.trim()).map_err(|_| format!("Invalid URL: {value}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("WebFetch only supports http and https URLs".into());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("WebFetch URLs must not include credentials".into());
    }
    // provider-visible WebFetch 约定把 HTTP 升级为 HTTPS 后再出站。
    if url.scheme() == "http" {
        url.set_scheme("https")
            .map_err(|_| format!("Invalid URL: {value}"))?;
    }
    if let Some(message) = blocked_host(&url) {
        return Err(message.into());
    }
    Ok(url)
}

/// TS `getBlockedHostReason`：URL 层只做稳定的形态过滤；IP 字面量交给每次 GET 前的出网拦截。
fn blocked_host(url: &Url) -> Option<&'static str> {
    let host = normalize_ip_literal(url.host_str().unwrap_or_default());
    if host.is_empty() {
        return Some("URL must include a hostname");
    }
    if parse_ip(&host).is_some() {
        return None;
    }
    if host == "localhost" || host.ends_with(".localhost") || host.ends_with(".local") {
        return Some("WebFetch requires a public hostname");
    }
    (host.split('.').count() < 2).then_some("Invalid URL")
}

/// TS `assertWebFetchLiteralEgress`：返回拦截文案，放行为 None。
pub fn egress_block(url: &Url) -> Option<&'static str> {
    let host = normalize_egress_host(url.host_str().unwrap_or_default());
    if host == "localhost" || host.ends_with(".localhost") {
        return Some("WebFetch cannot access private or local hostnames");
    }
    let address = parse_ip(&host)?;
    (!is_public(address)).then_some("WebFetch cannot access private or local IP addresses")
}

/// TS `resolveRedirectUrl`。
pub fn resolve_redirect(location: &str, current: &Url) -> Result<Url, String> {
    current
        .join(location)
        .map_err(|_| format!("Redirect Location is not a valid URL: {location}"))
}

/// TS `isPermittedRedirect`：无凭据、公网主机形态、同协议同端口、主机名忽略 `www.` 后相同。
pub fn redirect_permitted(from: &Url, to: &Url) -> bool {
    if !to.username().is_empty() || to.password().is_some() || blocked_host(to).is_some() {
        return false;
    }
    if from.scheme() != to.scheme() || from.port_or_known_default() != to.port_or_known_default() {
        return false;
    }
    let strip = |url: &Url| {
        let host = url.host_str().unwrap_or_default().to_lowercase();
        host.strip_prefix("www.").map(str::to_owned).unwrap_or(host)
    };
    strip(from) == strip(to)
}

/// TS `redactUrlCredentials`。
pub fn redact_credentials(url: &Url) -> String {
    let mut clone = url.clone();
    let _ = clone.set_username("");
    let _ = clone.set_password(None);
    clone.to_string()
}

/// TS `extractReadableContent`：非文本类型报错；HTML 转 Markdown，其余去首尾空白。
pub fn extract_readable(body: &[u8], content_type: &str) -> Result<String, String> {
    let mime = content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_lowercase();
    if !is_text_like(&mime) {
        let shown = if mime.is_empty() { "unknown" } else { &mime };
        return Err(format!("Unsupported WebFetch content type: {shown}"));
    }
    // TextDecoder 默认替换非法序列并去掉 BOM。
    let text = String::from_utf8_lossy(body);
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    if mime == "text/html" || mime == "application/xhtml+xml" || content_type.contains("html") {
        return html_to_markdown(text);
    }
    Ok(js_trim(text).to_owned())
}

/// TS `truncateContentForModel`（按 UTF-16 码元计长；切开代理对时以 U+FFFD 代替）。
pub fn truncate_for_model(content: &str) -> (String, bool) {
    if utf16_len(content) <= MAX_MODEL_INPUT_CHARS {
        return (content.to_owned(), false);
    }
    let max_body = MAX_MODEL_INPUT_CHARS.saturating_sub(utf16_len(TRUNCATION_SUFFIX));
    let mut body = String::new();
    let mut used = 0;
    for ch in content.chars() {
        let width = ch.len_utf16();
        if used + width > max_body {
            if used < max_body {
                body.push('\u{fffd}');
            }
            break;
        }
        body.push(ch);
        used += width;
    }
    (format!("{body}{TRUNCATION_SUFFIX}"), true)
}

/// TS `shouldReturnMarkdownDirectly`：预批准域名的短 Markdown 不经模型处理。
pub fn returns_markdown_directly(preapproved: bool, content_type: &str, content: &str) -> bool {
    preapproved
        && content_type.to_lowercase().contains("text/markdown")
        && utf16_len(content) < MAX_MODEL_INPUT_CHARS
}

/// TS `buildProcessingPrompt`。
pub fn processing_prompt(content: &str, prompt: &str, preapproved: bool) -> String {
    let instruction = if preapproved {
        "Provide a concise response based on the content above. Include relevant details, code examples, and documentation excerpts as needed.".to_owned()
    } else {
        [
            "Provide a concise response based only on the content above. In your response:",
            " - Enforce a strict 125-character maximum for quotes from any source document. Open Source Software is ok as long as we respect the license.",
            " - Use quotation marks for exact language from articles; any language outside of the quotation should never be word-for-word the same.",
            " - You are not a lawyer and never comment on the legality of your own prompts and responses.",
            " - Never produce or reproduce exact song lyrics.",
        ]
        .join("\n")
    };
    format!("\nWeb page content:\n---\n{content}\n---\n\n{prompt}\n\n{instruction}\n")
}

/// 模型返回的文本收口（TS：trim 后为空则给固定提示）。
pub fn processing_result(text: &str) -> String {
    let trimmed = js_trim(text);
    if trimmed.is_empty() {
        EMPTY_RESULT.to_owned()
    } else {
        trimmed.to_owned()
    }
}

/// TS `isWebFetchPreapprovedUrl`（与权限判定共用同一份生成名单）。
pub fn is_preapproved(url: &str) -> bool {
    crate::permission_rules::webfetch_preapproved("WebFetch", &serde_json::json!({ "url": url }))
}

pub fn utf16_len(value: &str) -> usize {
    value.encode_utf16().count()
}

/// JS `String.prototype.trim` 的空白集合（含 BOM，不含 U+0085）。
pub fn js_trim(value: &str) -> &str {
    value.trim_matches(|c: char| {
        matches!(
            c,
            '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'
                ..='\u{200a}'
                    | '\u{2028}'
                    | '\u{2029}'
                    | '\u{202f}'
                    | '\u{205f}'
                    | '\u{3000}'
                    | '\u{feff}'
        )
    })
}

fn is_text_like(mime: &str) -> bool {
    mime.is_empty()
        || mime.starts_with("text/")
        || matches!(
            mime,
            "application/json"
                | "application/xml"
                | "application/xhtml+xml"
                | "application/javascript"
                | "application/x-javascript"
        )
        || mime.ends_with("+json")
        || mime.ends_with("+xml")
}

fn html_to_markdown(html: &str) -> Result<String, String> {
    static RULES: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
        [
            (r"<!--[\s\S]*?-->", ""),
            (r"(?i)<script\b[\s\S]*?</script>", ""),
            (r"(?i)<style\b[\s\S]*?</style>", ""),
            (r"(?i)<noscript\b[\s\S]*?</noscript>", ""),
            (r"(?i)<h1\b[^>]*>([\s\S]*?)</h1>", "\n# ${1}\n"),
            (r"(?i)<h2\b[^>]*>([\s\S]*?)</h2>", "\n## ${1}\n"),
            (r"(?i)<h3\b[^>]*>([\s\S]*?)</h3>", "\n### ${1}\n"),
            (r"(?i)<h4\b[^>]*>([\s\S]*?)</h4>", "\n#### ${1}\n"),
            (r"(?i)<h5\b[^>]*>([\s\S]*?)</h5>", "\n##### ${1}\n"),
            (r"(?i)<h6\b[^>]*>([\s\S]*?)</h6>", "\n###### ${1}\n"),
            (
                r#"(?i)<a\b[^>]*href=["']([^"']+)["'][^>]*>([\s\S]*?)</a>"#,
                "[${2}](${1})",
            ),
            (r"(?i)<li\b[^>]*>([\s\S]*?)</li>", "\n- ${1}"),
            (r"(?i)<br\s*/?>", "\n"),
            (
                r"(?i)</(?:p|div|section|article|header|footer|tr|table|ul|ol)>",
                "\n",
            ),
            (r"<[^>]+>", ""),
        ]
        .into_iter()
        .map(|(pattern, replacement)| (Regex::new(pattern).expect("html rule"), replacement))
        .collect()
    });
    static SPACES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[ \t]+").expect("spaces"));
    let mut content = html.to_owned();
    for (rule, replacement) in RULES.iter() {
        content = rule.replace_all(&content, *replacement).into_owned();
    }
    let decoded = decode_entities(&content)?;
    let lines: Vec<String> = decoded
        .split('\n')
        .map(|line| js_trim(&SPACES.replace_all(line, " ")).to_owned())
        .collect();
    let kept: Vec<&str> = lines
        .iter()
        .enumerate()
        .filter(|(index, line)| {
            !line.is_empty()
                || index
                    .checked_sub(1)
                    .is_none_or(|prev| !lines[prev].is_empty())
        })
        .map(|(_, line)| line.as_str())
        .collect();
    Ok(js_trim(&kept.join("\n")).to_owned())
}

/// TS `decodeHtmlEntities`：按同样顺序逐个替换（`&amp;lt;` 会被解两次，与 TS 一致）。
fn decode_entities(value: &str) -> Result<String, String> {
    static HEX: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)&#x([0-9a-f]+);").expect("hex entity"));
    static DECIMAL: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"&#([0-9]+);").expect("decimal entity"));
    let mut text = value
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'");
    for (pattern, radix) in [(&*HEX, 16), (&*DECIMAL, 10)] {
        let mut failure = None;
        text = pattern
            .replace_all(&text, |captures: &regex::Captures| {
                let digits = &captures[1];
                match u32::from_str_radix(digits, radix) {
                    Ok(code) if code <= 0x10_ffff => {
                        char::from_u32(code).unwrap_or('\u{fffd}').to_string()
                    }
                    _ => {
                        failure.get_or_insert_with(|| {
                            let number = u128::from_str_radix(digits, radix)
                                .map(|n| n.to_string())
                                .unwrap_or_else(|_| digits.to_owned());
                            format!("Invalid code point {number}")
                        });
                        String::new()
                    }
                }
            })
            .into_owned();
        if let Some(message) = failure {
            return Err(message);
        }
    }
    Ok(text)
}

fn normalize_ip_literal(value: &str) -> String {
    let lower = value.to_lowercase();
    let lower = lower.strip_suffix('.').unwrap_or(&lower);
    let lower = lower.strip_prefix('[').unwrap_or(lower);
    lower.strip_suffix(']').unwrap_or(lower).to_owned()
}

fn normalize_egress_host(value: &str) -> String {
    let trimmed = value.trim().to_lowercase();
    let inner = if trimmed.starts_with('[') && trimmed.ends_with(']') {
        trimmed[1..trimmed.len() - 1].to_owned()
    } else {
        trimmed
    };
    inner.strip_suffix('.').map(str::to_owned).unwrap_or(inner)
}

#[cfg(test)]
#[path = "web_fetch_tests.rs"]
mod tests;
