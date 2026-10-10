//! Bash 参数级规则：对应 TS `bash-readonly-policy-argv-flags.ts` 与 `bash-readonly-policy-argv-direct.ts`。
use crate::bash_policy::{Policy, tables};

const XARGS_TARGETS: [&str; 8] = [
    "echo", "printf", "wc", "grep", "egrep", "fgrep", "head", "tail",
];

pub(crate) fn xargs_target(word: &str) -> bool {
    XARGS_TARGETS.contains(&word)
}

/// TS OPTION_PATTERN：`/^-[a-zA-Z0-9_-]/`。
fn option_like(w: &str) -> bool {
    let b = w.as_bytes();
    b.len() >= 2 && b[0] == b'-' && (b[1].is_ascii_alphanumeric() || b[1] == b'_' || b[1] == b'-')
}

fn all_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

pub(crate) fn allowed_by_policy(argv: &[String], p: &Policy, command: &str, start: usize) -> bool {
    if argv.is_empty() {
        return false;
    }
    if p.allow_any_args {
        return true;
    }
    if p.command_only {
        return argv.len() == start;
    }
    match &p.safe_flags {
        Some(flags) => flags_allowed(argv, p, flags, command, start),
        None => false,
    }
}

fn flags_allowed(
    argv: &[String],
    p: &Policy,
    flags: &std::collections::HashMap<String, String>,
    command: &str,
    start: usize,
) -> bool {
    let mut i = start;
    while i < argv.len() {
        let w = argv[i].as_str();
        if w.is_empty() {
            i += 1;
            continue;
        }
        if command == "xargs" && (!w.starts_with('-') || w == "--") {
            let target = if w == "--" {
                argv.get(i + 1).map(String::as_str)
            } else {
                Some(w)
            };
            return target.is_some_and(xargs_target);
        }
        if w == "--" {
            if p.respects_double_dash == Some(false) {
                i += 1;
                continue;
            }
            break;
        }
        let compact = w.len() > 1 && w.starts_with('-') && all_digits(&w[1..]);
        if compact && (command == "head" || command == "tail" || p.allow_compact_numeric) {
            i += 1;
            continue;
        }
        if w.len() > 1 && w.starts_with('-') && option_like(w) {
            let (flag, inline) = match w.find('=') {
                Some(eq) => (&w[..eq], Some(&w[eq + 1..])),
                None => (w, None),
            };
            let Some(kind) = flags.get(flag) else {
                // 短 flag 直接附值（如 -n5）。
                if !w.starts_with("--") && w.len() > 2 {
                    let short = &w[..2];
                    if let Some(kind) = flags.get(short).filter(|k| *k != "none") {
                        let value = &w[2..];
                        if !option_value_allowed(value, command, short)
                            || !kind_matches(value, kind)
                        {
                            return false;
                        }
                        i += 1;
                        continue;
                    }
                }
                if !flag.starts_with("--")
                    && flag.len() > 2
                    && flag[1..]
                        .chars()
                        .all(|c| flags.get(&format!("-{c}")).is_some_and(|k| k == "none"))
                {
                    i += 1;
                    continue;
                }
                return false;
            };
            match kind.as_str() {
                "none" => {
                    if inline.is_some() {
                        return false;
                    }
                    i += 1;
                }
                "optionalString" => i += 1,
                _ => {
                    let Some(value) = inline.or_else(|| argv.get(i + 1).map(String::as_str)) else {
                        return false;
                    };
                    if kind == "string"
                        && inline.is_none()
                        && !option_value_allowed(value, command, flag)
                    {
                        return false;
                    }
                    if !kind_matches(value, kind) {
                        return false;
                    }
                    i += if inline.is_some() { 1 } else { 2 };
                }
            }
            continue;
        }
        i += 1;
    }
    true
}

fn option_value_allowed(value: &str, command: &str, flag: &str) -> bool {
    if !value.starts_with('-') || value.len() <= 1 || !option_like(value) {
        return true;
    }
    command == "git" && flag == "--sort" && value.as_bytes()[1].is_ascii_alphabetic()
}

fn kind_matches(value: &str, kind: &str) -> bool {
    match kind {
        "number" => all_digits(value),
        "optionalString" | "string" => true,
        // JS length 按 UTF-16 计；单字符在 BMP 内时与 chars 计数一致。
        "char" => value.encode_utf16().count() == 1,
        "{}" => value == "{}",
        "EOF" => value == "EOF",
        _ => false,
    }
}

const FIND_WRITE: [&str; 10] = [
    "-delete",
    "-exec",
    "-execdir",
    "-files0-from",
    "-fls",
    "-fprint",
    "-fprint0",
    "-fprintf",
    "-ok",
    "-okdir",
];
const FIND_VALUE: [&str; 45] = [
    "-Bmin",
    "-Bnewer",
    "-Btime",
    "-D",
    "-amin",
    "-anewer",
    "-atime",
    "-cmin",
    "-cnewer",
    "-context",
    "-ctime",
    "-f",
    "-flags",
    "-fstype",
    "-gid",
    "-group",
    "-ilname",
    "-iname",
    "-inum",
    "-ipath",
    "-iregex",
    "-iwholename",
    "-lname",
    "-links",
    "-maxdepth",
    "-mindepth",
    "-mmin",
    "-mnewer",
    "-mtime",
    "-name",
    "-newer",
    "-path",
    "-perm",
    "-printf",
    "-regex",
    "-regextype",
    "-samefile",
    "-size",
    "-type",
    "-used",
    "-user",
    "-wholename",
    "-xattrname",
    "-xtype",
    "-uid",
];

pub(crate) fn is_find_write_option(w: &str) -> bool {
    FIND_WRITE.contains(&w)
}

const EXACT: [&[&str]; 5] = [
    &["ip", "addr"],
    &["node", "-v"],
    &["node", "--version"],
    &["python", "--version"],
    &["python3", "--version"],
];

pub(crate) fn direct(argv: &[String]) -> Option<bool> {
    if EXACT
        .iter()
        .any(|e| argv.len() == e.len() && e.iter().zip(argv).all(|(a, b)| a == b))
    {
        return Some(true);
    }
    let first = argv[0].as_str();
    if first == "docker"
        && tables()
            .allow_any_prefixes
            .iter()
            .any(|p| argv.len() >= p.len() && p.iter().zip(argv).all(|(a, b)| a == b))
    {
        return Some(!crate::bash_callbacks::docker_dangerous(argv));
    }
    let second = argv.get(1).map(String::as_str);
    match first {
        "printf" => Some(crate::bash_callbacks::printf_safe(argv)),
        "find" => Some(find_safe(argv)),
        "history" => Some(argv.len() == 1 || (argv.len() == 2 && all_digits(second.unwrap()))),
        "arch" => {
            Some(argv.len() == 1 || (argv.len() == 2 && matches!(second, Some("-h" | "--help"))))
        }
        "ifconfig" => Some(
            argv.len() == 1
                || (argv.len() == 2
                    && second
                        .unwrap()
                        .as_bytes()
                        .first()
                        .is_some_and(u8::is_ascii_alphabetic)),
        ),
        _ => None,
    }
}

fn find_safe(argv: &[String]) -> bool {
    let mut i = 1;
    while i < argv.len() {
        let w = argv[i].as_str();
        if is_find_write_option(w) {
            return false;
        }
        let newer = w.len() == 8
            && w.starts_with("-newer")
            && b"aBcm".contains(&w.as_bytes()[6])
            && b"aBcmt".contains(&w.as_bytes()[7]);
        if FIND_VALUE.contains(&w) || newer {
            i += 1;
        }
        i += 1;
    }
    true
}
