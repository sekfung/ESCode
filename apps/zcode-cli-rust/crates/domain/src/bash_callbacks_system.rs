//! Bash 系统类命令回调：printf、docker、lsof、ss，对应 TS `bash-readonly-policy-callbacks.ts` 同名函数。
use crate::bash_callbacks::re;
use regex::Regex;
use std::sync::OnceLock;

const DOCKER_FLAGS: [&str; 8] = [
    "-H",
    "-c",
    "--config",
    "--context",
    "--host",
    "--tlscacert",
    "--tlscert",
    "--tlskey",
];

pub(crate) fn docker_dangerous(args: &[String]) -> bool {
    args.iter().any(|a| {
        let direct = DOCKER_FLAGS.iter().any(|f| {
            a == f
                || a.strip_prefix(f).is_some_and(|r| r.starts_with('='))
                || (f.len() == 2 && a.len() > 2 && a.starts_with(f))
        });
        let short: String = a
            .strip_prefix('-')
            .map(|r| r.chars().take_while(char::is_ascii_alphabetic).collect())
            .unwrap_or_default();
        direct || (short.len() >= 2 && short.contains(['H', 'c']))
    })
}

pub(crate) fn printf_safe(argv: &[String]) -> bool {
    static OCTAL: OnceLock<Regex> = OnceLock::new();
    static UNI: OnceLock<Regex> = OnceLock::new();
    static NUMERIC: OnceLock<Regex> = OnceLock::new();
    static STAR: OnceLock<Regex> = OnceLock::new();
    static VALUE: OnceLock<Regex> = OnceLock::new();
    let second = argv.get(1).map(String::as_str);
    if second.is_some_and(|s| s.starts_with('-') && s != "--") {
        return false;
    }
    let fi = if second == Some("--") { 2 } else { 1 };
    let format = argv.get(fi).map_or("", String::as_str);
    if format.contains('$') {
        return false;
    }
    let f = format.replace("%%", "");
    if re(&OCTAL, r"%[^%a-zA-Z]*(?:hh|ll|[lLhqjzZt])?\\[0-7xX]").is_match(&f)
        || re(&UNI, r"\\[uU]").is_match(&f)
    {
        return false;
    }
    let numeric = re(
        &NUMERIC,
        r"%[-+ 0#']*[0-9.*]*(?:hh|ll|[lLhqjzZt])?[diouxXeEfFgGaAn]",
    )
    .is_match(&f);
    if numeric || re(&STAR, r"%[^%a-zA-Z]*\*").is_match(&f) {
        let value = re(
            &VALUE,
            r"^[-+]?(0[xX][0-9a-fA-F]+|[0-9]+#[0-9a-zA-Z]+|[0-9]*\.?[0-9]+([eE][-+]?[0-9]+)?)$",
        );
        for v in &argv[fi + 1..] {
            if v.contains('[') || v.contains('`') || v.contains("$(") || !value.is_match(v) {
                return false;
            }
        }
    }
    true
}

pub(crate) fn lsof_host_has_alpha(v: &str) -> bool {
    let host = v[v.find('@').unwrap() + 1..]
        .split(':')
        .next()
        .unwrap_or("");
    host.bytes().any(|b| b.is_ascii_alphabetic())
}

pub(crate) fn lsof(args: &[String]) -> bool {
    static ATTACHED: OnceLock<Regex> = OnceLock::new();
    static BARE: OnceLock<Regex> = OnceLock::new();
    for (i, a) in args.iter().enumerate() {
        if a.starts_with("+m") {
            return true;
        }
        if re(&ATTACHED, r"^-[a-zA-Z]*i\S*@").is_match(a) && lsof_host_has_alpha(a) {
            return true;
        }
        if re(&BARE, r"^-[a-zA-Z]*i$").is_match(a) {
            let next = args.get(i + 1).map_or("", String::as_str);
            if next.contains('@') && lsof_host_has_alpha(next) {
                return true;
            }
        }
    }
    false
}

pub(crate) fn ss(args: &[String]) -> bool {
    static KW: OnceLock<Regex> = OnceLock::new();
    static VKW: OnceLock<Regex> = OnceLock::new();
    static SPLIT: OnceLock<Regex> = OnceLock::new();
    let keywords = re(
        &KW,
        r"^(dst|src|dport|sport|and|or|not|eq|ne|ge|le|gt|lt|autobound|state|exclude|dev|fwmark|cgroup)$",
    );
    let value_kw = re(&VKW, r"^(state|exclude|dport|sport|dev|fwmark|cgroup)$");
    let mut positional = vec![];
    let mut after = false;
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        i += 1;
        if !after && a == "--" {
            after = true;
            continue;
        }
        if !after && a.starts_with('-') {
            if matches!(a, "-f" | "--family" | "-A" | "--query" | "--socket") {
                i += 1;
            }
            continue;
        }
        positional.push(a);
    }
    let joined = positional.join(" ");
    let mut skip = false;
    for token in re(&SPLIT, r"[\s()=!<>&|,]+")
        .split(&joined)
        .filter(|t| !t.is_empty())
    {
        if skip {
            skip = false;
            continue;
        }
        if keywords.is_match(token) {
            skip = value_kw.is_match(token);
            continue;
        }
        let high = token
            .bytes()
            .any(|b| matches!(b, b'g'..=b'z' | b'G'..=b'Z'));
        let hex = token
            .bytes()
            .any(|b| matches!(b, b'a'..=b'f' | b'A'..=b'F'));
        if high || (hex && (token.contains('.') || !token.contains(':'))) {
            return true;
        }
    }
    false
}
