//! Bash 稳定命令前缀解析：对应 TS `bash-command-permission-policy.ts` 的
//! resolveStableCommandPrefix 及其辅助（包装器剥离、选项跳过、深度覆盖）。
use crate::bash_parse::Invocation;
use serde_json::Value;
use std::sync::OnceLock;

const HIGH_RISK: [&str; 16] = [
    "bash",
    "chgrp",
    "chmod",
    "chown",
    "cmd",
    "dd",
    "fish",
    "mkfs",
    "mount",
    "powershell",
    "pwsh",
    "rm",
    "rmdir",
    "sh",
    "umount",
    "zsh",
];

const WRAPPER_OPTIONS: [&str; 4] = ["-p", "-v", "-V", "--ignore-environment"];

const ARG_IS_COMMAND: u64 = 1;

const ARG_IS_MODULE: u64 = 2;

pub(crate) fn wrapper_value_options(name: &str) -> Option<&'static [&'static str]> {
    Some(match name {
        "command" | "nohup" => &[],
        "env" => &[
            "-C",
            "-S",
            "-u",
            "--argv0",
            "--chdir",
            "--split-string",
            "--unset",
        ],
        "sudo" => &[
            "-C",
            "-D",
            "-R",
            "-T",
            "-a",
            "-c",
            "-g",
            "-h",
            "-p",
            "-r",
            "-t",
            "-u",
            "--askpass",
            "--chdir",
            "--chroot",
            "--close-from",
            "--group",
            "--host",
            "--prompt",
            "--role",
            "--type",
            "--user",
        ],
        "time" => &["-f", "-o", "--format", "--output"],
        _ => return None,
    })
}

pub(crate) fn registry() -> &'static Value {
    static R: OnceLock<Value> = OnceLock::new();
    R.get_or_init(|| {
        serde_json::from_str(include_str!("bash_command_registry.json"))
            .expect("generated registry")
    })
}

pub(crate) fn basename(token: &str) -> String {
    let n = token.replace('\\', "/");
    n[n.rfind('/').map_or(0, |i| i + 1)..].to_lowercase()
}

pub(crate) fn has_ws(s: &str) -> bool {
    s.chars().any(char::is_whitespace)
}

pub(crate) fn path_or_url(t: &str) -> bool {
    let b = t.as_bytes();
    t.contains("://")
        || t.starts_with("./")
        || t.starts_with("../")
        || t.starts_with('/')
        || t.starts_with('~')
        || (b.len() >= 3
            && b[0].is_ascii_alphabetic()
            && b[1] == b':'
            && (b[2] == b'\\' || b[2] == b'/'))
}

pub(crate) fn stable_action(t: Option<&String>) -> bool {
    t.is_some_and(|t| !t.is_empty() && !t.starts_with('-') && !path_or_url(t) && !has_ws(t))
}

pub(crate) fn stable_prefix(i: &Invocation) -> Option<String> {
    let assigns = assignments(i)?;
    if i.argv.is_empty() {
        return None;
    }
    let (executable, next, wrapper_prefix) = unwrap(&i.argv)?;
    let name = basename(&executable);
    if HIGH_RISK.contains(&name.as_str()) {
        return None;
    }
    let mut prefix: Vec<String> = assigns;
    prefix.extend(wrapper_prefix);
    prefix.push(executable);
    let remaining = &i.argv[next..];
    if let Some(o) = depth_override(&name, remaining) {
        prefix.extend(o);
        return serialize(&prefix);
    }
    let mut node = registry().get(&name)?;
    let skipped = skip_leading_options(node, remaining);
    if let Some(o) = depth_override(&name, &remaining[skipped..]) {
        prefix.extend(o);
        return serialize(&prefix);
    }
    let mut index = 0;
    let mut matched = false;
    while index < remaining.len() {
        if let Some(end) = skip_option(node, remaining, index) {
            index = end;
            continue;
        }
        let token = &remaining[index];
        let child = node[3]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c[0].as_array().unwrap().iter().any(|n| n == token.as_str()));
        if let Some(child) = child {
            prefix.push(token.clone());
            matched = true;
            node = child;
            index += 1;
            continue;
        }
        if node[2].as_u64().unwrap_or(0) & (ARG_IS_COMMAND | ARG_IS_MODULE) != 0
            && !path_or_url(token)
        {
            prefix.push(token.clone());
            matched = true;
        }
        break;
    }
    if matched { serialize(&prefix) } else { None }
}

pub(crate) fn skip_leading_options(node: &Value, args: &[String]) -> usize {
    let mut index = 0;
    while index < args.len() {
        match skip_option(node, args, index) {
            Some(end) => index = end,
            None => break,
        }
    }
    index
}

pub(crate) fn skip_option(node: &Value, args: &[String], index: usize) -> Option<usize> {
    let token = &args[index];
    if !token.starts_with('-') || token == "-" {
        return None;
    }
    if token == "--" {
        return Some(index + 1);
    }
    let name = token.split('=').next().unwrap();
    let option = node[1]
        .as_array()?
        .iter()
        .find(|o| o[0].as_array().unwrap().iter().any(|n| n == name))?;
    Some(
        index
            + if option[1] == 1 && !token.contains('=') {
                2
            } else {
                1
            },
    )
}

/// 返回（可执行名、其后参数下标、包装器前缀）；包装器超过两层时放弃。
pub(crate) fn unwrap(argv: &[String]) -> Option<(String, usize, Vec<String>)> {
    let mut prefix = vec![];
    let mut index = 0;
    let mut depth = 0;
    while index < argv.len() {
        let token = &argv[index];
        let name = basename(token);
        let Some(value_opts) = wrapper_value_options(&name) else {
            return Some((token.clone(), index + 1, prefix));
        };
        depth += 1;
        if depth > 2 {
            return None;
        }
        prefix.push(token.clone());
        index += 1;
        while index < argv.len() {
            let w = &argv[index];
            if name == "env" && static_assignment(w) {
                prefix.push(w.clone());
                index += 1;
                continue;
            }
            let option = w.split('=').next().unwrap();
            if value_opts.contains(&option) {
                index += if w.contains('=') { 1 } else { 2 };
                continue;
            }
            if WRAPPER_OPTIONS.contains(&option) || w.starts_with('-') {
                index += 1;
                continue;
            }
            break;
        }
    }
    None
}

pub(crate) fn depth_override(name: &str, args: &[String]) -> Option<Vec<String>> {
    let first = args.first().map(String::as_str);
    if matches!(name, "python" | "python3" | "py")
        && first == Some("-m")
        && stable_action(args.get(1))
    {
        return Some(args[..2].to_vec());
    }
    let script: &[&str] = match name {
        "bun" | "pnpm" | "yarn" => &["run"],
        "deno" => &["task"],
        "npm" => &["run", "run-script"],
        _ => &[],
    };
    if first.is_some_and(|f| script.contains(&f)) && stable_action(args.get(1)) {
        return Some(args[..2].to_vec());
    }
    if matches!(name, "just" | "make") && stable_action(args.first()) {
        return Some(args[..1].to_vec());
    }
    let depth = match (name, first) {
        ("aws" | "az", _) => 2,
        ("gcloud", _) => 3,
        ("docker", Some("compose")) | ("kubectl", Some("config")) => 2,
        _ => return None,
    };
    (args.len() >= depth && args[..depth].iter().all(|t| stable_action(Some(t))))
        .then(|| args[..depth].to_vec())
}

pub(crate) fn serialize(tokens: &[String]) -> Option<String> {
    (tokens.len() >= 2 && tokens.iter().all(|t| !t.is_empty() && !has_ws(t)))
        .then(|| tokens.join(" "))
}

pub(crate) fn static_assignment(token: &str) -> bool {
    let Some((name, value)) = token.split_once('=') else {
        return false;
    };
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_./:@,+-".contains(c))
}
pub(crate) fn assignments(i: &Invocation) -> Option<Vec<String>> {
    i.env
        .iter()
        .map(|(n, v)| {
            let token = format!("{n}={v}");
            (!n.is_empty() && static_assignment(&token)).then_some(token)
        })
        .collect()
}
