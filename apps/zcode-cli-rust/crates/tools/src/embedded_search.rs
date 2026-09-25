//! Bash 的 embedded search prelude：find → bfs、grep → ugrep、缺 rg 时补 rg 函数。
//! 逐条对齐 TS `resolveDefaultEmbeddedSearchBackend` 与 `buildEmbeddedSearchPreludeContent`，
//! 以及 `applyBashSourcesToExecutionRequest` 的落盘与 source 方式。见 docs/specs/rust-tool-surface.md。

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

const GREP_BYPASS_CASE_PATTERN: &str = "-*-filter*|-*-pager*|-*-view*|-*-format-open*|-*-config*|---*|-@*|-*-save-config*|-[Zz]*|-[!-]*[Zz]*|--null|--null-data";
const BFS_DEFAULT_ARGS: &str = "-S dfs -regextype findutils-default";
const UGREP_DEFAULT_ARGS: &str = "-G --ignore-files --hidden -I --exclude-dir=.git --exclude-dir=.svn --exclude-dir=.hg --exclude-dir=.bzr --exclude-dir=.jj --exclude-dir=.sl";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Backend {
    InternalCli {
        command: String,
        args: Vec<String>,
    },
    Native {
        find: String,
        grep: String,
        rg: String,
    },
}

/// TS `resolveDefaultEmbeddedSearchBackend`。
pub(crate) fn backend(env: &[(String, String)]) -> Backend {
    let get = |name: &str| {
        env.iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.trim().to_owned())
            .filter(|value| !value.is_empty())
    };
    if let Some(command) = get("ZCODE_EMBEDDED_SEARCH_COMMAND") {
        return Backend::InternalCli {
            command,
            args: vec!["__internal-search".to_owned()],
        };
    }
    Backend::Native {
        find: get("ZCODE_BFS_BINARY").unwrap_or_else(|| "bfs".to_owned()),
        grep: get("ZCODE_UGREP_BINARY").unwrap_or_else(|| "ugrep".to_owned()),
        rg: get("ZCODE_RG_BINARY").unwrap_or_else(|| "rg".to_owned()),
    }
}

/// TS `buildEmbeddedSearchPreludeContent`；`dialect` 取 TS 方言名（posix / git-bash / cmd / legacy-shell）。
pub(crate) fn prelude(backend: &Backend, dialect: &str) -> Option<String> {
    if matches!(dialect, "cmd" | "legacy-shell") {
        return None;
    }
    let git_bash = dialect == "git-bash";
    let backend = if git_bash {
        normalize_for_git_bash(backend)
    } else {
        backend.clone()
    };
    // Windows 不分发 bfs；Git Bash 保留系统 find。
    let wrap_find = !git_bash;
    let mut content = Vec::new();
    if wrap_find {
        content.push("unalias find 2>/dev/null || true".to_owned());
    }
    content.push("unalias grep 2>/dev/null || true".to_owned());
    if wrap_find {
        content.push(find_function(&backend));
    }
    content.push(grep_function(&backend));
    if let Some(fallback) = ripgrep_fallback(&backend) {
        content.push(fallback);
    }
    Some(content.join("\n"))
}

/// 与 TS 一致：内容写入 `<root>/bash-startup/<session>/embedded-search-startup-<sha256前16位>.sh`，
/// 命令前追加 `. '<path>'`（git-bash 用 POSIX 路径）。
pub(crate) async fn prepend_source(
    command: &str,
    content: &str,
    root: &Path,
    session: &str,
    git_bash: bool,
) -> anyhow::Result<String> {
    let dir = root.join("bash-startup").join(sanitize_segment(session));
    tokio::fs::create_dir_all(&dir).await?;
    let hash = format!("{:x}", Sha256::digest(content.as_bytes()));
    let path: PathBuf = dir.join(format!("embedded-search-startup-{}.sh", &hash[..16]));
    if tokio::fs::read_to_string(&path).await.ok().as_deref() != Some(content) {
        tokio::fs::write(&path, content).await?;
    }
    let native = path.to_string_lossy().into_owned();
    let source = if git_bash {
        let shell_path = windows_path_to_git_bash(&native);
        if shell_path == native {
            quote_always(&shell_path)
        } else {
            quote(&shell_path)
        }
    } else {
        quote_always(&native)
    };
    Ok(format!(". {source}\n{command}"))
}

fn find_function(backend: &Backend) -> String {
    match backend {
        Backend::InternalCli { command, .. } => function(
            "find",
            command,
            &format!("{} find \"$@\"", invocation(backend)),
        ),
        Backend::Native { find, .. } => function(
            "find",
            find,
            &format!("command {} {BFS_DEFAULT_ARGS} \"$@\"", quote(find)),
        ),
    }
}

fn grep_function(backend: &Backend) -> String {
    match backend {
        Backend::InternalCli { command, .. } => function(
            "grep",
            command,
            &format!("{} grep \"$@\"", invocation(backend)),
        ),
        Backend::Native { grep, .. } => function(
            "grep",
            grep,
            &format!("command {} {UGREP_DEFAULT_ARGS} \"$@\"", quote(grep)),
        ),
    }
}

fn ripgrep_fallback(backend: &Backend) -> Option<String> {
    // internal-cli 没有原生 rg；同名 rg 不是独立 fallback（外层已证明 shell 里没有 rg）。
    let Backend::Native { rg, .. } = backend else {
        return None;
    };
    if rg == "rg" {
        return None;
    }
    let body = function("rg", rg, &format!("command {} \"$@\"", quote(rg)))
        .lines()
        .map(|line| format!("  {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    Some(
        [
            "if ! (unalias rg 2>/dev/null; command -v rg) >/dev/null 2>&1; then",
            "  unalias rg 2>/dev/null || true",
            &body,
            "fi",
        ]
        .join("\n"),
    )
}

fn function(name: &str, backend_command: &str, invocation: &str) -> String {
    let mut lines = vec![format!("{name}() {{")];
    if name == "grep" {
        lines.push("  local _zcode_grep_arg".to_owned());
        lines.push("  for _zcode_grep_arg in \"$@\"; do".to_owned());
        lines.push(format!(
            "    case \"$_zcode_grep_arg\" in {GREP_BYPASS_CASE_PATTERN}) command grep \"$@\"; return ;; esac"
        ));
        lines.push("  done".to_owned());
    }
    lines.push(format!(
        "  command -v {} >/dev/null 2>&1 || {{ command {name} \"$@\"; return; }}",
        quote(backend_command)
    ));
    lines.push(format!("  {invocation}"));
    lines.push("}".to_owned());
    lines.join("\n")
}

fn invocation(backend: &Backend) -> String {
    let Backend::InternalCli { command, args } = backend else {
        unreachable!("only internal-cli backends use a command invocation");
    };
    std::iter::once("command".to_owned())
        .chain(std::iter::once(quote(command)))
        .chain(args.iter().map(|arg| quote(arg)))
        .collect::<Vec<_>>()
        .join(" ")
}

fn normalize_for_git_bash(backend: &Backend) -> Backend {
    let path = |value: &str| {
        if is_windows_absolute(value) {
            windows_path_to_git_bash(value)
        } else {
            value.to_owned()
        }
    };
    match backend {
        Backend::InternalCli { command, args } => Backend::InternalCli {
            command: path(command),
            args: args.iter().map(|arg| path(arg)).collect(),
        },
        Backend::Native { find, grep, rg } => Backend::Native {
            find: path(find),
            grep: path(grep),
            rg: path(rg),
        },
    }
}

fn is_windows_absolute(value: &str) -> bool {
    let bytes = value.as_bytes();
    (bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'/' | b'\\'))
        || value.starts_with("\\\\")
}

/// TS `windowsPathToGitBashPath`。
fn windows_path_to_git_bash(value: &str) -> String {
    if value.starts_with("\\\\") {
        return value.replace('\\', "/");
    }
    let bytes = value.as_bytes();
    if bytes.len() >= 2
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes.len() == 2 || matches!(bytes[2], b'/' | b'\\'))
    {
        let rest = value[2..].replace('\\', "/");
        let drive = (bytes[0] as char).to_ascii_lowercase();
        return if rest.starts_with('/') {
            format!("/{drive}{rest}")
        } else {
            format!("/{drive}/{rest}")
        };
    }
    value.replace('\\', "/")
}

fn quote(value: &str) -> String {
    let plain = !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_/:=.,@%+-".contains(c));
    if plain {
        value.to_owned()
    } else {
        quote_always(value)
    }
}

fn quote_always(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// TS `sanitizePathSegment`。
fn sanitize_segment(value: &str) -> String {
    let mut out = String::new();
    let mut in_run = false;
    for c in value.chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
            out.push(c);
            in_run = false;
        } else if !in_run {
            out.push('-');
            in_run = true;
        }
    }
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        "unknown".to_owned()
    } else {
        trimmed.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 语料由 scripts/generate-zcode-cli-rust-embedded-search-corpus.mjs 以 TS 为 oracle 生成。
    #[test]
    fn prelude_matches_ts_corpus() {
        let corpus: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/embedded_search_prelude.json"
        ))
        .unwrap();
        for case in corpus["cases"].as_array().unwrap() {
            let env: Vec<(String, String)> = case["env"]
                .as_object()
                .unwrap()
                .iter()
                .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_owned()))
                .collect();
            let dialect = case["dialect"].as_str().unwrap();
            assert_eq!(
                prelude(&backend(&env), dialect).as_deref(),
                case["content"].as_str(),
                "{env:?} {dialect}"
            );
        }
    }

    #[test]
    fn sanitizes_like_ts() {
        assert_eq!(sanitize_segment("sess_1/a b"), "sess_1-a-b");
        assert_eq!(sanitize_segment("///"), "unknown");
        assert_eq!(windows_path_to_git_bash("C:\\a b\\x.sh"), "/c/a b/x.sh");
        assert_eq!(windows_path_to_git_bash("D:"), "/d/");
    }
}
