//! WebSearch 纯规则（docs/specs/rust-websearch.md，对齐 TS `websearch.ts` 与 `websearch-results.ts`）。
//! 语料由 `scripts/generate-zcode-cli-rust-websearch-corpus.mjs` 从 TS oracle 生成。
use regex::Regex;
use serde_json::{Value, json};
use std::sync::LazyLock;

/// 会话侧请求 provider-native 搜索的内部工具标记（model adapter 负责线上编码）。
pub const MARKER: &str = "_zcode_native_web_search";
const DEFAULT_MAX_USES: u64 = 8;
const MAX_SOURCE_LINKS: usize = 20;
const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// TS `buildWebSearchProviderDescription`：`month` 为 1–12。
pub fn description(year: i32, month: u32) -> String {
    let name = MONTHS[(month.clamp(1, 12) - 1) as usize];
    [
        "Search the web. Returns result blocks with titles and URLs. US-only.".to_owned(),
        String::new(),
        format!(
            "- The current month is {name} {year} — use this when searching for recent information."
        ),
        "- `allowed_domains` / `blocked_domains` filter results.".to_owned(),
        "- After answering from results, end with a \"Sources:\" list of the URLs you used as markdown links.".to_owned(),
    ]
    .join("\n")
}

pub struct Input {
    pub query: String,
    pub allowed: Vec<String>,
    pub blocked: Vec<String>,
    pub max_uses: u64,
}

/// TS `WebSearchInputSchema`（strict）：query ≥ 2 字符，两个域名列表不能同时非空，maxUses 1–8。
pub fn validate(args: &Value) -> Result<Input, String> {
    let object = args.as_object().ok_or("WebSearch input must be an object")?;
    if let Some(key) = object.keys().find(|k| {
        !matches!(
            k.as_str(),
            "query" | "allowed_domains" | "blocked_domains" | "maxUses"
        )
    }) {
        return Err(format!("Unrecognized key: \"{key}\""));
    }
    let query = args["query"]
        .as_str()
        .filter(|q| q.encode_utf16().count() >= 2)
        .ok_or("query must be a string of at least 2 characters")?;
    let list = |key: &str| -> Result<Vec<String>, String> {
        match &args[key] {
            Value::Null => Ok(vec![]),
            Value::Array(items) => items
                .iter()
                .map(|v| v.as_str().map(str::to_owned))
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| format!("{key} must be an array of strings")),
            _ => Err(format!("{key} must be an array of strings")),
        }
    };
    let (allowed, blocked) = (list("allowed_domains")?, list("blocked_domains")?);
    if !allowed.is_empty() && !blocked.is_empty() {
        return Err("allowed_domains and blocked_domains cannot both be specified".into());
    }
    let max_uses = match &args["maxUses"] {
        Value::Null => DEFAULT_MAX_USES,
        value => value
            .as_u64()
            .filter(|n| (1..=DEFAULT_MAX_USES).contains(n))
            .ok_or("maxUses must be an integer from 1 to 8")?,
    };
    Ok(Input {
        query: query.to_owned(),
        allowed,
        blocked,
        max_uses,
    })
}

/// 内部搜索请求（TS webSearchHandler）：固定 system + user，唯一工具为 provider-native 搜索。
pub fn request(input: &Input) -> (Vec<Value>, Value) {
    let messages = vec![
        json!({"role":"system","content":"You are an assistant for performing a web search tool use."}),
        json!({"role":"user","content":format!("Perform a web search for the query: {}", input.query)}),
    ];
    let mut tool = json!({"type":MARKER,"max_uses":input.max_uses});
    if !input.allowed.is_empty() {
        tool["allowed_domains"] = json!(input.allowed);
    }
    if !input.blocked.is_empty() {
        tool["blocked_domains"] = json!(input.blocked);
    }
    (messages, tool)
}

/// TS `extractSourcesFromSummary`：markdown 链接（跳过图片），按 URL 小写去重。
fn sources(summary: &str) -> Vec<Value> {
    static LINK: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\[([^\]\n]+)\]\((https?://[^\s)]+)\)").unwrap());
    let mut seen = std::collections::HashSet::new();
    LINK.captures_iter(summary)
        .filter(|c| {
            let start = c.get(0).unwrap().start();
            !summary[..start].ends_with('!')
        })
        .filter_map(|c| {
            let url = c[2].trim().to_owned();
            let title = c[1].trim().to_owned();
            seen.insert(url.to_lowercase()).then(|| {
                let mut source = json!({"url":url});
                if !title.is_empty() {
                    source["title"] = title.into();
                }
                source
            })
        })
        .collect()
}

/// TS `buildWebSearchOutput`（流式收集：只有文本）。`durationMs` 由调用方补充。
pub fn output(query: &str, text: &str) -> Value {
    let summary = text.trim();
    let mut output = json!({"query":query,"results":[],"sources":sources(summary)});
    if !summary.is_empty() {
        output["summary"] = summary.into();
    }
    output
}

/// TS `formatWebSearchModelContent`。
pub fn model_content(output: &Value) -> String {
    let mut lines = vec![
        format!(
            "Web search results for query: \"{}\"",
            output["query"].as_str().unwrap_or_default()
        ),
        String::new(),
    ];
    if let Some(summary) = output["summary"].as_str() {
        lines.extend(["Summary:".to_owned(), summary.to_owned(), String::new()]);
    }
    let links = output["sources"].as_array().cloned().unwrap_or_default();
    lines.push("Links:".into());
    if links.is_empty() {
        lines.push("- No links found.".into());
    }
    for source in links.iter().take(MAX_SOURCE_LINKS) {
        let url = source["url"].as_str().unwrap_or_default();
        let title = source["title"].as_str().unwrap_or(url);
        lines.push(format!("- [{title}]({url})"));
    }
    lines.extend([
        String::new(),
        "REMINDER: You MUST include the sources above in your response to the user using markdown hyperlinks.".to_owned(),
    ]);
    lines.join("\n").trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn corpus() -> Value {
        serde_json::from_str(include_str!("../tests/fixtures/websearch_corpus.json")).unwrap()
    }

    #[test]
    fn description_matches_ts() {
        assert_eq!(description(2026, 9), corpus()["description"].as_str().unwrap());
    }

    #[test]
    fn outputs_and_model_content_match_ts() {
        for case in corpus()["outputs"].as_array().unwrap() {
            let text = case["text"].as_str().unwrap();
            let output = output("rust async", text);
            assert_eq!(output, case["output"], "{text:?}");
            assert_eq!(model_content(&output), case["content"].as_str().unwrap(), "{text:?}");
        }
    }

    #[test]
    fn validation_follows_the_ts_schema() {
        assert!(validate(&json!({"query":"a"})).is_err());
        assert!(validate(&json!({"query":"ab","allowed_domains":["x"],"blocked_domains":["y"]})).is_err());
        assert!(validate(&json!({"query":"ab","extra":1})).is_err());
        assert!(validate(&json!({"query":"ab","maxUses":9})).is_err());
        let input = validate(&json!({"query":"ab","allowed_domains":["x"]})).unwrap();
        assert_eq!(input.max_uses, 8);
        let (messages, tool) = request(&input);
        assert_eq!(messages[1]["content"], "Perform a web search for the query: ab");
        assert_eq!(tool, json!({"type":MARKER,"max_uses":8,"allowed_domains":["x"]}));
    }
}
