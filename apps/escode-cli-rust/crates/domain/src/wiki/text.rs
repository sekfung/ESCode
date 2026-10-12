//! 文本工具：分词、token 估算、slug、内容哈希。纯函数，检索与抽取共用同一套分词，保证索引与查询一致。
use sha2::{Digest, Sha256};

/// 英文停用词：只放高频虚词，避免把「must / never」这类规范信号词当作噪声。
const STOPWORDS: &[&str] = &[
    "a", "an", "and", "are", "as", "at", "be", "by", "for", "from", "has", "have", "in", "into",
    "is", "it", "its", "of", "on", "or", "that", "the", "this", "to", "was", "were", "will",
    "with", "we", "you", "i", "me", "my", "our", "your", "please", "can", "could", "would", "let",
    "us", "do", "does", "how", "what", "which", "about", "some", "any", "all", "also", "just",
];

pub fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// 存储用内容哈希：`sha256:<hex>`。
pub fn content_hash(bytes: &[u8]) -> String {
    format!("sha256:{}", sha256_hex(bytes))
}

pub fn short_hash(text: &str, len: usize) -> String {
    sha256_hex(text.as_bytes())[..len].to_owned()
}

/// 小写 ASCII slug；非 ASCII 字符（如中文标题）不进入 slug，由哈希段保证唯一。
pub fn slug(text: &str, max: usize) -> String {
    let mut out = String::new();
    let mut dash = false;
    for c in text.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
        if out.len() >= max {
            break;
        }
    }
    let trimmed = out.trim_matches('-').to_owned();
    if trimmed.is_empty() {
        "entry".into()
    } else {
        trimmed
    }
}

fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x3040..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xAC00..=0xD7AF | 0xF900..=0xFAFF)
}

/// 分词：ASCII 词按驼峰/下划线/数字边界再拆（同时保留整词），CJK 连续段取二元组（单字段保留单字）。
pub fn tokenize(text: &str) -> Vec<String> {
    let mut tokens = vec![];
    let mut word = String::new();
    let mut cjk = String::new();
    let flush_word = |word: &mut String, tokens: &mut Vec<String>| {
        if word.is_empty() {
            return;
        }
        push_word(word, tokens);
        word.clear();
    };
    let flush_cjk = |cjk: &mut String, tokens: &mut Vec<String>| {
        let chars: Vec<char> = cjk.chars().collect();
        match chars.len() {
            0 => {}
            1 => tokens.push(chars[0].to_string()),
            _ => tokens.extend(chars.windows(2).map(|w| w.iter().collect::<String>())),
        }
        cjk.clear();
    };
    for c in text.chars() {
        if is_cjk(c) {
            flush_word(&mut word, &mut tokens);
            cjk.push(c);
        } else if c.is_alphanumeric() {
            flush_cjk(&mut cjk, &mut tokens);
            word.push(c);
        } else {
            flush_word(&mut word, &mut tokens);
            flush_cjk(&mut cjk, &mut tokens);
        }
    }
    flush_word(&mut word, &mut tokens);
    flush_cjk(&mut cjk, &mut tokens);
    tokens
}

/// 轻量英文词干：复数与常见时态后缀（errors→error、returned→return、caching→cach），
/// 索引与查询走同一函数，只要求两侧一致，不追求语言学正确。
pub fn stem(word: &str) -> String {
    if !word.is_ascii() || word.len() <= 3 || word.chars().any(|c| c.is_ascii_digit()) {
        return word.to_owned();
    }
    let strip = |suffix: &str, min: usize| word.strip_suffix(suffix).filter(|w| w.len() >= min);
    if let Some(w) = strip("ies", 2) {
        return format!("{w}y");
    }
    for suffix in ["sses", "xes", "ches", "shes"] {
        if let Some(w) = strip(suffix, 2) {
            return format!("{w}{}", &suffix[..suffix.len() - 2]);
        }
    }
    if let Some(w) = strip("ing", 3) {
        return w.to_owned();
    }
    if let Some(w) = strip("ed", 3) {
        return w.to_owned();
    }
    if word.ends_with('s')
        && !word.ends_with("ss")
        && !word.ends_with("us")
        && !word.ends_with("is")
    {
        return word[..word.len() - 1].to_owned();
    }
    word.to_owned()
}

fn push_word(word: &str, tokens: &mut Vec<String>) {
    let lower = word.to_lowercase();
    let mut parts = vec![];
    let mut current = String::new();
    let chars: Vec<char> = word.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        let boundary = i > 0 && {
            let prev = chars[i - 1];
            (c.is_uppercase() && prev.is_lowercase())
                || (c.is_ascii_digit() != prev.is_ascii_digit())
                || (c.is_uppercase()
                    && prev.is_uppercase()
                    && chars.get(i + 1).is_some_and(|n| n.is_lowercase()))
        };
        if boundary && !current.is_empty() {
            parts.push(std::mem::take(&mut current));
        }
        current.extend(c.to_lowercase());
    }
    if !current.is_empty() {
        parts.push(current);
    }
    let keep = |t: &str| t.chars().count() > 1 || t.chars().all(|c| c.is_ascii_digit());
    if !STOPWORDS.contains(&lower.as_str()) && keep(&lower) {
        tokens.push(stem(&lower));
    }
    if parts.len() > 1 {
        for part in parts {
            if part != lower && !STOPWORDS.contains(&part.as_str()) && keep(&part) {
                tokens.push(stem(&part));
            }
        }
    }
}

/// token 估算：CJK 每字约 1 token，其余约 4 字符 1 token（与 core 的保守估计同量级，只用于预算）。
pub fn estimate_tokens(text: &str) -> usize {
    let (mut cjk, mut other) = (0usize, 0usize);
    for c in text.chars() {
        if is_cjk(c) { cjk += 1 } else { other += 1 }
    }
    cjk + other.div_ceil(4)
}

/// 按字符截断（不切断 UTF-8），超出时加省略号。
pub fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// 空白归一：连续空白折成一个空格，保留段落换行。
pub fn normalize_whitespace(text: &str) -> String {
    text.lines()
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_owned()
}
