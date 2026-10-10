//! `-p` 参数解析（docs/specs/rust-headless-prompt.md）：对齐 Node `arguments.ts` 的 `util.parseArgs({strict: true})`
//! 与 `extractDisallowedToolsArgs`，错误文本逐字一致。
use std::path::PathBuf;

/// Node 全局选项表：(长名, 短名, 是否取值, 是否可重复)。Node 认识但 Rust `-p` 不支持的选项单列在 [`UNSUPPORTED`]。
const OPTIONS: &[(&str, Option<char>, bool, bool)] = &[
    ("help", Some('h'), false, false),
    ("json", None, false, false),
    ("output-format", None, true, false),
    ("no-color", None, false, false),
    ("no-browser", None, false, false),
    ("browser-use", None, true, false),
    ("browser-executable", None, true, false),
    ("prompt", Some('p'), true, false),
    ("memory-bench", None, false, false),
    ("enable-workflow", None, false, false),
    ("attach", None, true, true),
    ("cwd", None, true, false),
    ("locale", None, true, false),
    ("resume", None, true, false),
    ("target", None, true, false),
    ("target-replace", None, false, false),
    ("continue", Some('c'), false, false),
    ("force", Some('f'), false, false),
    ("force-mcs", None, false, false),
    ("mode", None, true, false),
    ("verbose", None, false, false),
    ("version", Some('v'), false, false),
    ("prepare-storage", None, false, false),
    ("stdio", None, false, false),
    ("surface", None, true, false),
    ("all", Some('a'), false, false),
    ("available", None, false, false),
    ("keep-data", None, false, false),
    ("scope", Some('s'), true, false),
    ("sparse", None, true, true),
    // Rust 独有：数据目录与单模型配置（同 app-server）。
    ("data-dir", None, true, false),
    ("config", None, true, false),
];

/// Node 的 `-p` 支持、Rust 本期不支持的选项（明确报错，不静默忽略）。
const UNSUPPORTED: &[&str] = &[
    "target",
    "target-replace",
    "browser-use",
    "browser-executable",
    "memory-bench",
    "enable-workflow",
    "force-mcs",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputFormat {
    Text,
    Json,
}

#[derive(Debug, Default)]
pub struct Parsed {
    pub help: bool,
    pub version: bool,
    pub prompt: Option<String>,
    pub output: Option<OutputFormat>,
    pub mode: Option<String>,
    pub cwd: Option<PathBuf>,
    pub attach: Vec<String>,
    pub resume: Option<String>,
    pub continue_session: bool,
    pub disallowed_tools: Vec<String>,
    pub desktop: bool,
    pub verbose: bool,
    pub data_dir: Option<PathBuf>,
    pub config: Option<PathBuf>,
}

/// 解析失败：`usage` 为 true 时 Node 会在错误后空一行打印帮助（parseArgs 类错误）。
#[derive(Debug, PartialEq, Eq)]
pub struct ArgError {
    pub message: String,
    pub usage: bool,
}

fn usage(message: impl Into<String>) -> ArgError {
    ArgError {
        message: message.into(),
        usage: true,
    }
}

fn plain(message: impl Into<String>) -> ArgError {
    ArgError {
        message: message.into(),
        usage: false,
    }
}

/// Node `isCliOptionToken`。
fn is_option_token(value: &str) -> bool {
    value.starts_with('-')
}

/// Node `extractDisallowedToolsArgs`：贪婪读取后续非选项参数；`--flag=value` 形式直接取值。
fn extract_disallowed(argv: Vec<String>) -> Result<(Vec<String>, Vec<String>), ArgError> {
    let (mut rest, mut values) = (vec![], vec![]);
    let mut iter = argv.into_iter().peekable();
    while let Some(arg) = iter.next() {
        if let Some(value) = ["--disallowedTools=", "--disallowed-tools="]
            .iter()
            .find_map(|prefix| arg.strip_prefix(prefix))
        {
            values.push(value.to_owned());
            continue;
        }
        if arg != "--disallowedTools" && arg != "--disallowed-tools" {
            rest.push(arg);
            continue;
        }
        let mut consumed = false;
        while let Some(next) = iter.next_if(|next| !is_option_token(next)) {
            values.push(next);
            consumed = true;
        }
        if !consumed {
            return Err(usage(format!("{arg} requires at least one tool.")));
        }
    }
    Ok((rest, normalize_tool_rules(&values)))
}

/// Node `normalizeCliToolRuleList`：逗号或空白分隔（括号内不拆），去重，`web_search` 归一为 `WebSearch`。
fn normalize_tool_rules(values: &[String]) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for value in values {
        let (mut current, mut in_args) = (String::new(), false);
        let mut rules = vec![];
        for ch in value.chars() {
            match ch {
                '(' => {
                    in_args = true;
                    current.push(ch);
                }
                ')' => {
                    in_args = false;
                    current.push(ch);
                }
                ',' | ' ' if !in_args => rules.push(std::mem::take(&mut current)),
                _ => current.push(ch),
            }
        }
        rules.push(current);
        for rule in rules.iter().map(|r| r.trim()).filter(|r| !r.is_empty()) {
            let rule = if rule == "web_search" {
                "WebSearch".to_owned()
            } else if let Some(rest) = rule.strip_prefix("web_search(") {
                format!("WebSearch({rest}")
            } else {
                rule.to_owned()
            };
            if !out.contains(&rule) {
                out.push(rule);
            }
        }
    }
    out
}

fn option_label(long: &str, short: Option<char>) -> String {
    match short {
        Some(short) => format!("-{short}, --{long} <value>"),
        None => format!("--{long} <value>"),
    }
}

fn ambiguous(token: &str, long: &str, short: Option<char>) -> ArgError {
    let hint = match (token.starts_with("--"), short) {
        (false, Some(short)) => format!("'--{long}=-XYZ' or '-{short}-XYZ'"),
        _ => format!("'--{long}=-XYZ'"),
    };
    usage(format!(
        "Option '{token}' argument is ambiguous.\nDid you forget to specify the option argument for '{token}'?\nTo specify an option argument starting with a dash use {hint}."
    ))
}

fn unknown(token: &str) -> ArgError {
    usage(format!(
        "Unknown option '{token}'. To specify a positional argument starting with a '-', place it at the end of the command after '--', as in '-- \"{token}\""
    ))
}

/// 原始解析结果：长名 → 值列表（布尔选项值为空串）。
type Values = Vec<(&'static str, String)>;

/// `util.parseArgs` strict 子集：长选项（`--x`、`--x=v`）、短选项（`-x v`、`-xv`、布尔组合 `-cv`）、`--` 终止。
fn parse_args(argv: &[String]) -> Result<(Values, Vec<String>), ArgError> {
    let by_long = |name: &str| OPTIONS.iter().find(|o| o.0 == name);
    let by_short = |c: char| OPTIONS.iter().find(|o| o.1 == Some(c));
    let (mut values, mut positionals) = (Values::new(), vec![]);
    let mut i = 0;
    while i < argv.len() {
        let token = &argv[i];
        i += 1;
        if token == "--" {
            positionals.extend(argv[i..].iter().cloned());
            break;
        }
        if let Some(body) = token.strip_prefix("--") {
            let (name, inline) = match body.split_once('=') {
                Some((name, value)) => (name, Some(value.to_owned())),
                None => (body, None),
            };
            let &(long, short, takes, _) =
                by_long(name).ok_or_else(|| unknown(&format!("--{name}")))?;
            let value = match (takes, inline) {
                (false, Some(_)) => {
                    return Err(usage(format!(
                        "Option '--{long}' does not take an argument"
                    )));
                }
                (false, None) => String::new(),
                (true, Some(value)) => value,
                (true, None) => match argv.get(i) {
                    None => {
                        return Err(usage(format!(
                            "Option '{}' argument missing",
                            option_label(long, short)
                        )));
                    }
                    Some(next) if is_option_token(next) => {
                        return Err(ambiguous(token, long, short));
                    }
                    Some(next) => {
                        i += 1;
                        next.clone()
                    }
                },
            };
            values.push((long, value));
            continue;
        }
        if let Some(group) = token.strip_prefix('-').filter(|g| !g.is_empty()) {
            let chars: Vec<char> = group.chars().collect();
            let mut j = 0;
            while j < chars.len() {
                let &(long, short, takes, _) =
                    by_short(chars[j]).ok_or_else(|| unknown(&format!("-{}", chars[j])))?;
                j += 1;
                if !takes {
                    values.push((long, String::new()));
                    continue;
                }
                // `-px`：短选项后的剩余字符即值。
                let attached: String = chars[j..].iter().collect();
                let value = if !attached.is_empty() {
                    attached
                } else {
                    match argv.get(i) {
                        None => {
                            return Err(usage(format!(
                                "Option '{}' argument missing",
                                option_label(long, short)
                            )));
                        }
                        Some(next) if is_option_token(next) => {
                            return Err(ambiguous(&format!("-{}", chars[j - 1]), long, short));
                        }
                        Some(next) => {
                            i += 1;
                            next.clone()
                        }
                    }
                };
                values.push((long, value));
                break;
            }
            continue;
        }
        positionals.push(token.clone());
    }
    Ok((values, positionals))
}

/// 完整解析与校验（顺序同 Node run.ts）。
pub fn parse(argv: Vec<String>) -> Result<Parsed, ArgError> {
    let (argv, disallowed_tools) = extract_disallowed(argv)?;
    let (values, positionals) = parse_args(&argv)?;
    let last = |name: &str| {
        values
            .iter()
            .rev()
            .find(|(k, _)| *k == name)
            .map(|(_, v)| v.clone())
    };
    let has = |name: &str| values.iter().any(|(k, _)| *k == name);
    let mut parsed = Parsed {
        help: has("help"),
        version: has("version"),
        prompt: last("prompt"),
        cwd: last("cwd").map(PathBuf::from),
        attach: values
            .iter()
            .filter(|(k, _)| *k == "attach")
            .map(|(_, v)| v.clone())
            .collect(),
        resume: last("resume"),
        continue_session: has("continue"),
        disallowed_tools,
        verbose: has("verbose"),
        data_dir: last("data-dir").map(PathBuf::from),
        config: last("config").map(PathBuf::from),
        ..Parsed::default()
    };
    if let Some(locale) = last("locale")
        && !matches!(locale.as_str(), "en-US" | "zh-CN" | "auto")
    {
        return Err(plain(format!(
            "Unsupported --locale value: {locale}. Supported locales: en-US, zh-CN, auto."
        )));
    }
    if let Some(mode) = last("mode") {
        let lower = mode.to_lowercase();
        if !matches!(lower.as_str(), "build" | "plan" | "edit" | "yolo") {
            return Err(plain(format!(
                "Unsupported --mode value: {mode}. Supported modes: build, edit, plan, yolo."
            )));
        }
        parsed.mode = Some(lower);
    }
    if let Some(surface) = last("surface") {
        parsed.desktop = match surface.as_str() {
            "terminal" => false,
            "desktop" => true,
            _ => {
                return Err(plain(format!(
                    "Unsupported --surface value: {surface}. Supported surfaces: terminal, desktop."
                )));
            }
        };
    }
    if parsed.continue_session && parsed.resume.is_some() {
        return Err(plain("--resume and --continue cannot be used together."));
    }
    parsed.output = match last("output-format").as_deref() {
        None => has("json").then_some(OutputFormat::Json),
        Some("text") => Some(OutputFormat::Text),
        Some("json") => Some(OutputFormat::Json),
        Some("stream-json") => {
            return Err(plain(
                "--output-format stream-json is not supported by the Rust runtime yet.",
            ));
        }
        Some(other) => {
            return Err(plain(format!(
                "--output-format must be one of text, json, stream-json (received: {other})."
            )));
        }
    };
    if let Some(option) = UNSUPPORTED.iter().find(|name| has(name)) {
        return Err(plain(format!(
            "--{option} is not supported by the Rust runtime."
        )));
    }
    // Node：-p 存在时多余的位置参数忽略；无 -p 的位置参数是子命令，Rust 只有 app-server。
    if parsed.prompt.is_none()
        && let Some(command) = positionals.first()
        && !parsed.help
        && !parsed.version
    {
        return Err(usage(format!("Unknown command: {command}")));
    }
    Ok(parsed)
}

#[cfg(test)]
#[path = "args_tests.rs"]
mod tests;
