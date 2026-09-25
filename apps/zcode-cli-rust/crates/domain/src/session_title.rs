//! 会话标题规则（对齐 TS `runtime/methods/session-title.ts`、
//! `title-generation-sidecar.ts` 与 `helpers/project.ts#titleFromInput`）。
//!
//! 纯规则：首条输入标题公式、生成请求的输入归一、生成标题的清洗与解析。语料由
//! `scripts/generate-zcode-cli-rust-title-corpus.mjs` 从 TS oracle 生成，见
//! `docs/specs/rust-session-title.md`。
use super::custom_command::{js_trim, js_ws, truncate_utf16};

/// 生成请求的 system prompt（生成资产，与 TS 逐字一致）。
pub const SYSTEM_PROMPT: &str = include_str!("session_title_prompt.txt");
pub const MAX_TITLE_INPUT_CHARS: usize = 1_200;
pub const MAX_TITLE_CHARS: usize = 100;
/// 短输入门槛（TS `MIN_GENERATED_TITLE_INPUT_CHARS`）：按 code point 计。
pub const MIN_GENERATED_TITLE_INPUT_CHARS: usize = 10;

/// 标题 sidecar 的 seed：首条输入的可见文本与其实体 id（会话回退后放弃写回）。
#[derive(Clone)]
pub struct TitleSeed {
    pub entity: String,
    pub text: String,
    /// `/goal` 等外部输入入口同为用户真实意图，不受 10 字符门槛限制。
    pub bypass_short_guard: bool,
}

/// `shouldAttemptSessionTitleGeneration` 的纯规则部分；parent/taskType/尝试次数由会话 owner 判定。
pub fn should_generate(seed: &TitleSeed) -> bool {
    let normalized = normalize_title_input(&seed.text);
    !normalized.is_empty() && (seed.bypass_short_guard || passes_short_guard(&normalized))
}

/// 生成请求的 messages（TS `buildTitleMessages`）。
pub fn request_messages(seed: &TitleSeed) -> Vec<serde_json::Value> {
    vec![
        serde_json::json!({"role":"system","content":SYSTEM_PROMPT}),
        serde_json::json!({"role":"user","content":normalize_title_input(&seed.text)}),
    ]
}

/// JS `String#replace(/\s+/g, " ")`。
fn collapse_whitespace(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut space = false;
    for c in value.chars() {
        if js_ws(c) {
            space = true;
            continue;
        }
        if space && !out.is_empty() {
            out.push(' ');
        }
        space = false;
        out.push(c);
    }
    out
}

/// `titleFromInput`：首条输入的可读标题，最长 60 个 UTF-16 码元。
pub fn title_from_input(input: &str) -> String {
    let compact = collapse_whitespace(js_trim(input));
    if compact.is_empty() {
        return "Untitled session".into();
    }
    if compact.encode_utf16().count() <= 60 {
        compact
    } else {
        format!("{}...", truncate_utf16(&compact, 57))
    }
}

/// `normalizeTitleInput`：生成请求的 user 内容。
pub fn normalize_title_input(input: &str) -> String {
    truncate_utf16(&collapse_whitespace(js_trim(input)), MAX_TITLE_INPUT_CHARS)
}

/// 短输入门槛：归一后按 code point 计数（TS `Array.from(...).length`）。
pub fn passes_short_guard(normalized: &str) -> bool {
    normalized.chars().count() >= MIN_GENERATED_TITLE_INPUT_CHARS
}

/// ASCII 大小写不敏感查找（needle 只含 ASCII；UTF-8 续字节 ≥ 0x80 不会误配）。
fn find_ignore_ascii_case(haystack: &str, needle: &str, from: usize) -> Option<usize> {
    let bytes = haystack.as_bytes();
    if bytes.len() < needle.len() || from > bytes.len() - needle.len() {
        return None;
    }
    (from..=bytes.len() - needle.len())
        .find(|index| bytes[*index..*index + needle.len()].eq_ignore_ascii_case(needle.as_bytes()))
}

/// JS `raw.replace(/ thinking[\s\S]*?<\/think>/gi, "")`：未闭合的 ` thinking` 不参与匹配。
fn strip_thinking(raw: &str) -> String {
    // 尖括号用 \u{3c}/\u{3e} 转义书写：直接写字面量会被工具链改写（见 docs/specs/rust-session-title.md）。
    const OPEN: &str = "\u{3c}think\u{3e}";
    const CLOSE: &str = "\u{3c}/think\u{3e}";
    let mut out = String::with_capacity(raw.len());
    let mut index = 0;
    while let Some(start) = find_ignore_ascii_case(raw, OPEN, index) {
        let Some(end) = find_ignore_ascii_case(raw, CLOSE, start + OPEN.len()) else {
            // 该开标签之后没有闭标签：连同标签一起保留，继续找后面的成对标签。
            out.push_str(&raw[index..start + OPEN.len()]);
            index = start + OPEN.len();
            continue;
        };
        out.push_str(&raw[index..start]);
        index = end + CLOSE.len();
    }
    out.push_str(&raw[index..]);
    out
}

/// `firstNonEmptyLine`：按 `\r?\n` 切行、逐行 trim 后取首个非空行。
fn first_non_empty_line(text: &str) -> Option<String> {
    text.split('\n')
        .map(|line| js_trim(line.strip_suffix('\r').unwrap_or(line)))
        .find(|line| !line.is_empty())
        .map(str::to_owned)
}

/// 整段 JSON：对象且 `title` 为字符串（可为空串，调用方按空值处理）。
fn parse_title_json_candidate(text: &str) -> Option<String> {
    let parsed: serde_json::Value = serde_json::from_str(text).ok()?;
    if !parsed.is_object() {
        return None;
    }
    parsed.get("title")?.as_str().map(str::to_owned)
}

/// `extractFencedJson`：```` ```json … ``` ```` 内层内容，两端 trim。
fn extract_fenced_json(text: &str) -> Option<String> {
    let text = js_trim(text);
    let body = text.strip_prefix("```")?;
    let body = body.trim_start_matches([' ', '\t']);
    let body = match body.get(..4).filter(|tag| tag.eq_ignore_ascii_case("json")) {
        Some(_) => &body[4..],
        None => body,
    };
    let body = body.trim_start_matches([' ', '\t']);
    let body = body
        .strip_prefix("\r\n")
        .or_else(|| body.strip_prefix('\n'))?;
    let body = body.strip_suffix("```")?;
    // 正则的 `\r?\n?` 优先吃掉紧邻结尾围栏的换行；其余由 trim 收口。
    let body = body
        .strip_suffix("\r\n")
        .or_else(|| body.strip_suffix('\n'))
        .unwrap_or(body);
    Some(js_trim(body).to_owned())
}

/// `parseTitleJson`：先整段 JSON，再围栏 JSON；返回首个解析出的 `title`。
fn parse_title_json(text: &str) -> Option<String> {
    let mut candidates = Vec::new();
    if !text.is_empty() {
        candidates.push(text.to_owned());
    }
    if let Some(fenced) = extract_fenced_json(text).filter(|value| !value.is_empty()) {
        candidates.push(fenced);
    }
    candidates
        .iter()
        .find_map(|candidate| parse_title_json_candidate(candidate))
}

/// `cleanGeneratedTitle`：清洗模型输出；不含字母数字/CJK 时视为无效。
pub fn clean_generated_title(raw: &str) -> Option<String> {
    let without_thinking = strip_thinking(raw);
    let text = js_trim(&without_thinking);
    let candidate = match parse_title_json(text) {
        Some(parsed) => parsed,
        None => first_non_empty_line(text)?,
    };
    if candidate.is_empty() {
        return None;
    }
    // `.replace(/^#+\s*/, "")`
    let without_heading = {
        let rest = candidate.trim_start_matches('#');
        rest.trim_start_matches(js_ws)
    };
    // `.replace(/^[\s"'`“”‘’]+|[\s"'`“”‘’]+$/g, "")`
    let quoted = |c: char| {
        js_ws(c) || matches!(c, '"' | '\'' | '`' | '\u{201c}' | '\u{201d}' | '\u{2018}' | '\u{2019}')
    };
    let unquoted = without_heading.trim_matches(quoted);
    // `.replace(/[.。!！?？:：,，;；]+$/g, "")`
    let unpunctuated = unquoted.trim_end_matches(['.', '。', '!', '！', '?', '？', ':', '：', ',', '，', ';', '；']);
    let collapsed = collapse_whitespace(unpunctuated);
    let cleaned = js_trim(&collapsed);
    let has_word = cleaned.chars().any(|c| {
        c.is_ascii_alphanumeric() || matches!(c, '\u{3400}'..='\u{9fff}')
    });
    if !has_word {
        return None;
    }
    if cleaned.encode_utf16().count() > MAX_TITLE_CHARS {
        let truncated = truncate_utf16(cleaned, MAX_TITLE_CHARS - 3);
        return Some(format!("{}...", js_trim(&truncated)));
    }
    Some(cleaned.to_owned())
}

