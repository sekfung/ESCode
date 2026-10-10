//! Bash git 子命令回调：对应 TS `bash-readonly-policy-git-callbacks.ts`。
use crate::bash_callbacks::re;
use regex::Regex;
use std::sync::OnceLock;

pub(crate) fn git_revision_format(args: &[String]) -> bool {
    static SIG: OnceLock<Regex> = OnceLock::new();
    for (i, a) in args.iter().enumerate() {
        let value = match a.find('=') {
            Some(eq) => Some(&a[eq + 1..]),
            None => args.get(i + 1).map(String::as_str),
        };
        let is_fmt = a == "--format"
            || a == "--pretty"
            || a.starts_with("--format=")
            || a.starts_with("--pretty=");
        if is_fmt
            && value
                .is_some_and(|v| !v.is_empty() && re(&SIG, r"%[-+ ]?G|%\(\*?signature").is_match(v))
        {
            return true;
        }
    }
    false
}

pub(crate) fn git_ls_remote(args: &[String]) -> bool {
    let mut after = false;
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        i += 1;
        if !after && a == "--" {
            after = true;
            continue;
        }
        if !after && (a.starts_with('-') || a.is_empty()) {
            if a == "--sort" {
                i += 1;
            }
            continue;
        }
        return true;
    }
    false
}

pub(crate) fn git_remote_show(args: &[String]) -> bool {
    let dd = args.iter().position(|a| a == "--");
    let options = &args[..dd.unwrap_or(args.len())];
    let mut positional: Vec<&String> = dd.map_or(vec![], |d| args[d + 1..].iter().collect());
    positional.extend(options.iter().filter(|a| *a != "-n"));
    if !options.iter().any(|a| a == "-n") || positional.len() != 1 {
        return true;
    }
    let p = positional[0].as_bytes();
    !(p.first()
        .is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_')
        && p.iter()
            .all(|c| c.is_ascii_alphanumeric() || *c == b'_' || *c == b'-'))
}

pub(crate) fn list_like(args: &[String], value_flags: &[&str]) -> bool {
    let (mut list, mut after) = (false, false);
    let mut previous = String::new();
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        i += 1;
        if a.is_empty() {
            continue;
        }
        if a == "--" && !after {
            after = true;
            previous.clear();
            continue;
        }
        if !after && a.starts_with('-') {
            if a == "--list" || a == "-l" || (!a.starts_with("--") && a[1..].contains('l')) {
                list = true;
            }
            previous = a.split('=').next().unwrap().to_owned();
            if !a.contains('=') && value_flags.contains(&previous.as_str()) {
                i += 1;
            }
            continue;
        }
        if !list && previous != "--merged" && previous != "--no-merged" {
            return true;
        }
    }
    false
}
