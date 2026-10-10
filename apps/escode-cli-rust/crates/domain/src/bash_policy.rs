//! Bash 只读策略：逐位对应 TS `bash-readonly-policy-argv*.ts`。策略表来自生成器导出的
//! `bash_policies.json`，回调见 bash_callbacks.rs。入口 `is_readonly` 对应 `isRuntimeReadOnlyBashCommand`
//! （不含带工作目录上下文的 git 运行时检查，见 docs/specs/rust-permission-modes.md）。
use crate::bash_parse::{Invocation, analyze};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

pub(crate) struct Policy {
    /// None 与空表语义不同：TS 无 safeFlags 字段时直接拒绝，空对象仍允许位置参数。
    pub safe_flags: Option<HashMap<String, String>>,
    pub allow_any_args: bool,
    pub allow_compact_numeric: bool,
    pub command_only: bool,
    pub respects_double_dash: Option<bool>,
    pub regex: Option<regex::Regex>,
    pub callback: Option<String>,
}

pub(crate) struct Tables {
    pub commands: HashMap<String, Policy>,
    pub multiword: Vec<(String, Policy)>,
    pub git: Vec<(String, Policy)>,
    pub allow_any: HashSet<String>,
    pub allow_any_prefixes: Vec<Vec<String>>,
    pub git_no_value: HashSet<String>,
    pub git_value: HashSet<String>,
    pub git_dangerous: Vec<String>,
}

fn policy(v: &Value) -> Policy {
    Policy {
        safe_flags: v["safeFlags"].as_object().map(|m| {
            m.iter()
                .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_owned()))
                .collect()
        }),
        allow_any_args: v["allowAnyArgs"] == true,
        allow_compact_numeric: v["allowCompactNumericCountFlag"] == true,
        command_only: v["commandOnly"] == true,
        respects_double_dash: v["respectsDoubleDash"].as_bool(),
        regex: v["regex"]["source"]
            .as_str()
            .map(|s| regex::Regex::new(s).expect("generated Bash policy regex")),
        callback: v["callback"].as_str().map(str::to_owned),
    }
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|s| s.as_str().unwrap().to_owned())
        .collect()
}

/// TS 按前缀词数降序做稳定排序；同长度保持表内原序。
fn ordered(v: &Value) -> Vec<(String, Policy)> {
    let mut entries: Vec<(String, Policy)> = v
        .as_array()
        .unwrap()
        .iter()
        .map(|e| (e[0].as_str().unwrap().to_owned(), policy(&e[1])))
        .collect();
    entries.sort_by_key(|(k, _)| std::cmp::Reverse(k.split(' ').count()));
    entries
}

pub(crate) fn tables() -> &'static Tables {
    static TABLES: OnceLock<Tables> = OnceLock::new();
    TABLES.get_or_init(|| {
        let j: Value = serde_json::from_str(include_str!("bash_policies.json"))
            .expect("generated Bash policies");
        Tables {
            commands: j["commands"]
                .as_array()
                .unwrap()
                .iter()
                .map(|e| (e[0].as_str().unwrap().to_owned(), policy(&e[1])))
                .collect(),
            multiword: ordered(&j["multiword"]),
            git: ordered(&j["gitSubcommands"]),
            allow_any: strings(&j["allowAnyArgCommands"]).into_iter().collect(),
            allow_any_prefixes: j["allowAnyArgCommandPrefixes"]
                .as_array()
                .unwrap()
                .iter()
                .map(strings)
                .collect(),
            git_no_value: strings(&j["gitGlobalNoValueFlags"]).into_iter().collect(),
            git_value: strings(&j["gitGlobalValueFlags"]).into_iter().collect(),
            git_dangerous: strings(&j["gitGlobalDangerousFlags"]),
        }
    })
}

/// TS `isRuntimeReadOnlyBashCommand(command)`（无运行时上下文）。
pub fn is_readonly(command: &str) -> bool {
    is_readonly_with_git_context(command, false)
}

/// 带运行时上下文的分类：`git_context_unsafe` 由 adapter（需要文件系统）判定后传入，
/// domain 只做纯决策（TS `isRuntimeReadOnlyBashCommand(command, context)` 把两者合在一起）。
pub fn is_readonly_with_git_context(command: &str, git_context_unsafe: bool) -> bool {
    let analysis = analyze(command);
    if !analysis.permission_safe() || analysis.commands.is_empty() {
        return false;
    }
    let names: Vec<&str> = analysis
        .commands
        .iter()
        .map(|c| {
            strip_wrappers(&c.argv)
                .first()
                .map_or(c.name.as_str(), String::as_str)
        })
        .collect();
    let has_git = names.contains(&"git");
    // git 可能在目标目录加载 hooks/config；与 cd/pushd/popd 同时出现时不放行。
    if has_git && names.iter().any(|n| matches!(*n, "cd" | "pushd" | "popd")) {
        return false;
    }
    if has_git && git_context_unsafe {
        return false;
    }
    let mut any = false;
    for part in &analysis.commands {
        if has_known_write_option(part) {
            return false;
        }
        match evaluate(part) {
            Some(true) => any = true,
            _ => return false,
        }
    }
    any
}

const SAFE_ENV: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "BLOCK_SIZE",
    "BLOCKSIZE",
    "CGO_ENABLED",
    "CHARSET",
    "CI",
    "CLICOLOR",
    "CLICOLOR_FORCE",
    "COLORTERM",
    "COLUMNS",
    "DEBIAN_FRONTEND",
    "FORCE_COLOR",
    "GCC_COLORS",
    "GIT_TERMINAL_PROMPT",
    "GO111MODULE",
    "GOARCH",
    "GOEXPERIMENT",
    "GOOS",
    "GREP_COLOR",
    "GREP_COLORS",
    "LANG",
    "LANGUAGE",
    "LC_ALL",
    "LC_CTYPE",
    "LC_TIME",
    "LINES",
    "LSCOLORS",
    "LS_COLORS",
    "NO_COLOR",
    "NODE_ENV",
    "PYTEST_DEBUG",
    "PYTEST_DISABLE_PLUGIN_AUTOLOAD",
    "PYTHONDONTWRITEBYTECODE",
    "PYTHONUNBUFFERED",
    "RUST_BACKTRACE",
    "RUST_LOG",
    "TERM",
    "TIME_STYLE",
    "TZ",
];

fn redirects_allowed(part: &Invocation) -> bool {
    part.redirects.iter().all(|r| {
        let t = r.target.as_str();
        if t.starts_with("/dev/tcp/") || t.starts_with("/dev/udp/") {
            return false;
        }
        if r.op == ">&" && !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit()) {
            return true;
        }
        if t == "/dev/null" {
            return true;
        }
        matches!(r.op.as_str(), "<" | "<<" | "<&" | "<<<") && !is_unc(t)
    })
}

pub(crate) fn is_unc(v: &str) -> bool {
    let b = v.as_bytes();
    b.len() >= 3
        && ((b[0] == b'/' && b[1] == b'/') || (b[0] == b'\\' && b[1] == b'\\'))
        && b[2] != b'/'
        && b[2] != b'\\'
}

pub(crate) fn strip_wrappers(argv: &[String]) -> &[String] {
    let mut s = argv;
    loop {
        match s.first().map(String::as_str) {
            Some("command") => {
                let mut i = 1;
                while s.get(i).is_some_and(|w| {
                    w.len() > 1 && w.starts_with('-') && w[1..].bytes().all(|b| b == b'p')
                }) {
                    i += 1;
                }
                if s.get(i).is_some_and(|w| w == "--") {
                    i += 1;
                }
                if i >= s.len() || s[i].starts_with('-') {
                    return s;
                }
                s = &s[i..];
            }
            Some("builtin") => {
                let i = if s.get(1).is_some_and(|w| w == "--") {
                    2
                } else {
                    1
                };
                if i >= s.len() {
                    return s;
                }
                s = &s[i..];
            }
            Some("noglob") if s.len() > 1 => s = &s[1..],
            _ => return s,
        }
    }
}

fn has_known_write_option(part: &Invocation) -> bool {
    let argv = strip_wrappers(&part.argv);
    match argv.first().map(String::as_str) {
        Some("sed") => argv
            .iter()
            .any(|w| crate::bash_callbacks::is_sed_in_place(w)),
        Some("find") => argv
            .iter()
            .any(|w| crate::bash_policy_argv::is_find_write_option(w)),
        Some("tree") => tree_has_output(argv),
        Some("git") => argv
            .iter()
            .any(|w| crate::bash_policy_git::git_dangerous_word(w)),
        _ => false,
    }
}

fn tree_has_output(argv: &[String]) -> bool {
    for w in &argv[1..] {
        if w.is_empty() {
            continue;
        }
        if w == "--" {
            return false;
        }
        if w == "-o" || w == "--output" || w.starts_with("--output=") {
            return true;
        }
        if w.starts_with('-') && !w.starts_with("--") && w[1..].contains('o') {
            return true;
        }
    }
    false
}

fn evaluate(part: &Invocation) -> Option<bool> {
    use crate::bash_policy_argv::{allowed_by_policy, direct};
    if !part
        .env
        .iter()
        .all(|(n, _)| !n.is_empty() && SAFE_ENV.contains(&n.as_str()))
    {
        return Some(false);
    }
    if !redirects_allowed(part) {
        return Some(false);
    }
    let argv = strip_wrappers(&part.argv);
    if argv.is_empty() || argv.iter().any(|w| is_unc(w)) {
        return Some(false);
    }
    if argv[0] == "git" {
        return Some(crate::bash_policy_git::git_readonly(argv));
    }
    if let Some(r) = direct(argv) {
        return Some(r);
    }
    let t = tables();
    for (prefix, p) in &t.multiword {
        let words: Vec<&str> = prefix.split(' ').collect();
        if !words
            .iter()
            .enumerate()
            .all(|(i, w)| argv.get(i).is_some_and(|a| a == w))
        {
            continue;
        }
        let args = &argv[words.len()..];
        if args
            .iter()
            .any(|a| a.contains('$') || (a.contains('{') && (a.contains(',') || a.contains(".."))))
        {
            return Some(false);
        }
        if p.callback
            .as_deref()
            .is_some_and(|cb| crate::bash_callbacks::dangerous(cb, prefix, args))
        {
            return Some(false);
        }
        if !allowed_by_policy(argv, p, &argv[0], words.len()) {
            return Some(false);
        }
        return Some(
            p.regex
                .as_ref()
                .is_none_or(|r| r.is_match(&part.command_text)),
        );
    }
    if t.allow_any.contains(&argv[0]) {
        return Some(true);
    }
    if cfg!(windows) && argv[0] == "xargs" {
        return None;
    }
    let p = t.commands.get(&argv[0])?;
    if argv[0] == "cd" && argv.len() > 2 {
        return Some(false);
    }
    if p.callback
        .as_deref()
        .is_some_and(|cb| crate::bash_callbacks::dangerous(cb, &part.command_text, &argv[1..]))
    {
        return Some(false);
    }
    if !allowed_by_policy(argv, p, &argv[0], 1) {
        return Some(false);
    }
    Some(
        p.regex
            .as_ref()
            .is_none_or(|r| r.is_match(&part.command_text)),
    )
}
