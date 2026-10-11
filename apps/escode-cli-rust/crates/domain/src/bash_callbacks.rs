//! Bash 策略回调：逐个对应 TS `bash-readonly-policy-callbacks.ts` 与 `bash-readonly-policy-git-callbacks.ts`。
//! 名称来自生成器导出的 `callback` 字段；未知名称按危险处理（生成器 --check 保证两端同步）。
use crate::bash_callbacks_git as git;
use crate::bash_callbacks_system as sys;
use regex::Regex;
use std::sync::OnceLock;
pub(crate) use sys::{docker_dangerous, printf_safe};

pub(crate) fn re(cell: &'static OnceLock<Regex>, src: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(src).expect("static regex"))
}

pub(crate) fn dangerous(name: &str, text: &str, args: &[String]) -> bool {
    let _ = text;
    match name {
        "jqCommandIsDangerous" => jq(args),
        "sedCommandIsDangerous" => sed(args),
        "dateCommandIsDangerous" => date(args),
        "psCommandIsDangerous" => args.iter().any(|a| {
            !a.starts_with('-') && a.bytes().all(|b| b.is_ascii_alphabetic()) && a.contains('e')
        }),
        "pyrightCommandIsDangerous" => args.iter().any(|a| a == "--watch" || a == "-w"),
        "manCommandIsDangerous" => man(args),
        "lsofCommandIsDangerous" => sys::lsof(args),
        "tputCommandIsDangerous" => tput(args),
        "ssCommandIsDangerous" => sys::ss(args),
        "testCommandIsDangerous" => test_cmd(args),
        "xargsCommandIsDangerous" => xargs(args),
        "ghCommandIsDangerous" => gh(args),
        "dockerCommandIsDangerous" => sys::docker_dangerous(args),
        "gitRevisionFormatCommandIsDangerous" => git::git_revision_format(args),
        "gitReflogCommandIsDangerous" => {
            let first = args.iter().find(|a| !a.is_empty() && !a.starts_with('-'));
            first.is_some_and(|f| f != "show" && f != "list")
                || args.iter().any(|a| {
                    matches!(
                        a.as_str(),
                        "expire" | "delete" | "exists" | "drop" | "write"
                    )
                })
        }
        "gitLsRemoteCommandIsDangerous" => git::git_ls_remote(args),
        "gitRemoteShowCommandIsDangerous" => git::git_remote_show(args),
        "gitTagCommandIsDangerous" => git::list_like(
            args,
            &[
                "--contains",
                "--no-contains",
                "--merged",
                "--no-merged",
                "--points-at",
                "--sort",
                "--format",
                "-n",
            ],
        ),
        "gitBranchCommandIsDangerous" => git::list_like(
            args,
            &["--contains", "--no-contains", "--points-at", "--sort"],
        ),
        "inline:git remote" => args.iter().any(|a| a != "-v" && a != "--verbose"),
        _ => true,
    }
}

pub(crate) fn is_sed_in_place(w: &str) -> bool {
    w.starts_with("-i") || w == "--in-place" || w.starts_with("--in-place=")
}

fn jq(args: &[String]) -> bool {
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        if a.is_empty() {
            i += 1;
            continue;
        }
        if a.starts_with("-f")
            || a.starts_with("-L")
            || [
                "--argfile",
                "--from-file",
                "--library-path",
                "--rawfile",
                "--run-tests",
                "--slurpfile",
            ]
            .iter()
            .any(|f| a == *f || a.strip_prefix(f).is_some_and(|r| r.starts_with('=')))
        {
            return true;
        }
        if a == "--" {
            return jq_filter(args.get(i + 1).map_or("", String::as_str));
        }
        if a == "--indent" {
            i += 2;
            continue;
        }
        if a.starts_with('-') {
            i += 1;
            continue;
        }
        return jq_filter(a);
    }
    false
}

/// `/\$ENV\b/`、`/(^|[^A-Za-z0-9_$.])env(?=$|[^A-Za-z0-9_])/`、include/import 同理（Rust regex 无前瞻，手写边界）。
fn jq_filter(f: &str) -> bool {
    static ENV: OnceLock<Regex> = OnceLock::new();
    if re(&ENV, r"\$ENV\b").is_match(f) {
        return true;
    }
    let word = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    let b = f.as_bytes();
    let bounded = |kw: &str, extra: &[u8]| {
        f.match_indices(kw).any(|(i, _)| {
            let before_ok = i == 0 || !(word(b[i - 1]) || extra.contains(&b[i - 1]));
            let after = i + kw.len();
            before_ok && (after == b.len() || !word(b[after]))
        })
    };
    bounded("env", b"$.") || bounded("include", b"") || bounded("import", b"")
}

fn sed_writes(script: &str) -> bool {
    static W: OnceLock<Regex> = OnceLock::new();
    re(&W, r"(?:^|[;{\n])\s*(?:[0-9,$!+~-]+)?\s*w(?:\s|$)").is_match(script)
}

fn sed(args: &[String]) -> bool {
    let mut first_script = false;
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        i += 1;
        if a.is_empty() {
            continue;
        }
        if is_sed_in_place(a) {
            return true;
        }
        if a == "-e" || a == "--expression" {
            if sed_writes(args.get(i).map_or("", String::as_str)) {
                return true;
            }
            i += 1;
            continue;
        }
        if let Some(s) = a.strip_prefix("--expression=") {
            if sed_writes(s) {
                return true;
            }
            continue;
        }
        if a == "-l" || a == "--line-length" {
            i += 1;
            continue;
        }
        if a.starts_with("--line-length=") {
            continue;
        }
        if a == "--" {
            return sed_writes(args.get(i).map_or("", String::as_str));
        }
        if !a.starts_with('-') && !first_script {
            first_script = true;
            if sed_writes(a) {
                return true;
            }
        }
    }
    false
}

fn date(args: &[String]) -> bool {
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        if a.starts_with("--") && a.contains('=') {
            i += 1;
        } else if a.starts_with('-') {
            i += if matches!(a, "-d" | "--date" | "-r" | "--reference" | "--rfc-3339") {
                2
            } else {
                1
            };
        } else if !a.starts_with('+') {
            return true;
        } else {
            i += 1;
        }
    }
    false
}

fn man(args: &[String]) -> bool {
    let (mut apropos, mut after) = (false, false);
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        i += 1;
        if !after && a == "--" {
            after = true;
            continue;
        }
        if !after && a.starts_with('-') && a != "-" {
            apropos |= matches!(a, "-k" | "-f" | "--apropos" | "--whatis");
            if a == "-S" || a == "-s" {
                i += 1;
            }
            continue;
        }
        after = true;
        if a.contains('/') {
            return !apropos;
        }
    }
    false
}

const TPUT_DANGEROUS: [&str; 24] = [
    "clear", "flash", "if", "init", "iprog", "is1", "is2", "is3", "mc0", "mc4", "mc5", "mc5i",
    "mc5p", "pfkey", "pfloc", "pfx", "pfxl", "reset", "rf", "rmcup", "rs1", "rs2", "rs3", "smcup",
];

fn tput(args: &[String]) -> bool {
    let mut after = false;
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        if a == "--" {
            after = true;
            i += 1;
            continue;
        }
        if !after && a.starts_with('-') {
            if a == "-S" || (!a.starts_with("--") && a.len() > 2 && a.contains('S')) {
                return true;
            }
            i += if a == "-T" { 2 } else { 1 };
            continue;
        }
        if TPUT_DANGEROUS.contains(&a) {
            return true;
        }
        i += 1;
    }
    false
}

fn test_cmd(args: &[String]) -> bool {
    static NUM: OnceLock<Regex> = OnceLock::new();
    let safe =
        |v: &str| re(&NUM, r"^-?(0[xX][0-9a-fA-F]+|[0-9]+#[0-9a-zA-Z]+|[0-9]+)$").is_match(v);
    if args
        .iter()
        .any(|a| matches!(a.as_str(), "-v" | "-R" | "-a" | "-o") || a.contains('['))
    {
        return true;
    }
    for (i, a) in args.iter().enumerate() {
        if matches!(a.as_str(), "-eq" | "-ne" | "-lt" | "-le" | "-gt" | "-ge") {
            let prev = i.checked_sub(1).and_then(|p| args.get(p));
            if [prev, args.get(i + 1)]
                .into_iter()
                .flatten()
                .any(|v| !safe(v))
            {
                return true;
            }
        }
        if a == "-t" && args.get(i + 1).is_some_and(|v| !safe(v)) {
            return true;
        }
    }
    false
}

fn xargs(args: &[String]) -> bool {
    let mut i = 0;
    while i < args.len() {
        let mut a = args[i].as_str();
        if a.is_empty() {
            i += 1;
            continue;
        }
        if a == "--" && i + 1 < args.len() {
            i += 1;
            a = args[i].as_str();
        }
        if a.starts_with('-') && a != "-" {
            if matches!(a, "-I" | "-n" | "-P" | "-L" | "-s" | "-E" | "-d") {
                i += 1;
            }
            i += 1;
            continue;
        }
        return !crate::bash_policy_argv::xargs_target(a);
    }
    false
}

fn gh(args: &[String]) -> bool {
    for a in args {
        if a.is_empty() {
            continue;
        }
        let mut value = a.as_str();
        if a.starts_with('-') {
            match a.find('=') {
                Some(eq) if eq + 1 < a.len() => value = &a[eq + 1..],
                _ => continue,
            }
        }
        if !value.contains('/') && !value.contains('@') {
            continue;
        }
        if value.contains("://") || value.contains('@') || value.matches('/').count() >= 2 {
            return true;
        }
    }
    false
}
