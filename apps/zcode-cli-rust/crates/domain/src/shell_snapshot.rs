//! Bash 的 shell 初始化快照脚本与 cwd 捕获（docs/specs/rust-bash-shell-snapshot.md），逐字对齐 TS
//! `adapters/src/exec/shell-init-snapshot.ts`、`cwd-capture.ts` 与 `core/.../bash-cwd-policy.ts`。纯函数。

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShellKind {
    Bash,
    Zsh,
    Sh,
}

impl ShellKind {
    /// TS `detectShellKind`：按 shell 文件名判断。
    pub fn detect(shell_path: &str) -> Self {
        let name = shell_path.rsplit(['/', '\\']).next().unwrap_or(shell_path).to_lowercase();
        if name.contains("zsh") {
            Self::Zsh
        } else if name.contains("bash") {
            Self::Bash
        } else {
            Self::Sh
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Bash => "bash",
            Self::Zsh => "zsh",
            Self::Sh => "sh",
        }
    }
    /// TS `detectShellInitConfigPath` 的文件名部分。
    pub fn config_file(self) -> &'static str {
        match self {
            Self::Zsh => ".zshrc",
            Self::Bash => ".bashrc",
            Self::Sh => ".profile",
        }
    }
}

/// TS `buildShellInitSnapshotCreationScript`。
pub fn creation_script(config_exists: bool, config_path: &str, path_value: &str, kind: ShellKind, snapshot_path: &str) -> String {
    let mut lines: Vec<String> = vec![
        format!("SNAPSHOT_FILE={}", quote_always(snapshot_path)),
        if config_exists {
            format!("source {} < /dev/null", double_quote(config_path))
        } else {
            "# No user config file to source".into()
        },
        String::new(),
        "# First, create/clear the snapshot file".into(),
        r##"echo "# Snapshot file" >| "$SNAPSHOT_FILE""##.into(),
        String::new(),
        "# When this file is sourced, we first unalias to avoid conflicts".into(),
        r##"# This is necessary because aliases get "frozen" inside function definitions at definition time,"##.into(),
        "# which can cause unexpected behavior when functions use commands that conflict with aliases".into(),
        r##"echo "# Unset all aliases to avoid conflicts with functions" >> "$SNAPSHOT_FILE""##.into(),
        r##"echo "unalias -a 2>/dev/null || true" >> "$SNAPSHOT_FILE""##.into(),
        String::new(),
    ];
    if config_exists {
        lines.extend(export_lines(kind).iter().map(|s| s.to_string()));
    } else if kind != ShellKind::Zsh {
        lines.push(r##"echo "shopt -s expand_aliases" >> "$SNAPSHOT_FILE""##.into());
    }
    lines.extend([
        String::new(),
        "# Add PATH to the file".into(),
        "cat >> \"$SNAPSHOT_FILE\" << 'PATH_END_ZCODE_SHELL_INIT_SNAPSHOT'".into(),
        format!("export PATH={}", quote_always(path_value)),
        "PATH_END_ZCODE_SHELL_INIT_SNAPSHOT".into(),
        String::new(),
        "# Exit silently on success, only report errors".into(),
        r##"if [ ! -f "$SNAPSHOT_FILE" ]; then"##.into(),
        r##"  echo "Error: Snapshot file was not created at $SNAPSHOT_FILE" >&2"##.into(),
        "  exit 1".into(),
        "fi".into(),
    ]);
    lines.join("\n")
}

/// TS 原文：`set -o | grep "on" | awk '{print "set -o " $1}' | head -n 1000 >> "$SNAPSHOT_FILE"`。
pub const TS_SET_O_LINE: &str = r##"set -o | grep "on" | awk '{print "set -o " $1}' | head -n 1000 >> "$SNAPSHOT_FILE""##;
/// 只写入状态为 on 的选项（docs/specs/rust-bash-shell-snapshot.md 的有意差异）。
pub const TS_SET_O_LINE_FIXED: &str = r##"set -o | awk '$2 == "on" {print "set -o " $1}' | head -n 1000 >> "$SNAPSHOT_FILE""##;

const ALIAS_LINES: [&str; 9] = [
    r##"echo "# Aliases" >> "$SNAPSHOT_FILE""##,
    r##"# Filter out winpty aliases on Windows to avoid "stdin is not a tty" errors"##,
    r##"# Git Bash automatically creates aliases like "alias node='winpty node.exe'" for"##,
    "# programs that need Win32 Console in mintty, but winpty fails when there's no TTY",
    r##"if [[ "$OSTYPE" == "msys" ]] || [[ "$OSTYPE" == "cygwin" ]]; then"##,
    r##"  alias | grep -v "='winpty " | sed 's/^alias //g' | sed 's/^/alias -- /' | head -n 1000 >> "$SNAPSHOT_FILE""##,
    "else",
    r##"  alias | sed 's/^alias //g' | sed 's/^/alias -- /' | head -n 1000 >> "$SNAPSHOT_FILE""##,
    "fi",
];

/// TS `snapshotExportLines`。
fn export_lines(kind: ShellKind) -> Vec<&'static str> {
    let mut lines: Vec<&'static str> = if kind == ShellKind::Zsh {
        vec![
            r##"echo "# Functions" >> "$SNAPSHOT_FILE""##,
            "",
            "# Force autoload all functions first",
            "typeset -f > /dev/null 2>&1",
            "",
            "# Now get user function names - filter completion functions (single underscore prefix)",
            "# but keep double-underscore helpers (e.g. __zsh_like_cd from mise, __pyenv_init)",
            "typeset +f | grep -vE '^_[^_]' | while read func; do",
            r##"  typeset -f "$func" >> "$SNAPSHOT_FILE""##,
            "done",
            "",
            r##"echo "# Shell Options" >> "$SNAPSHOT_FILE""##,
            r##"setopt | sed 's/^/setopt /' | head -n 1000 >> "$SNAPSHOT_FILE""##,
        ]
    } else {
        vec![
            r##"echo "# Functions" >> "$SNAPSHOT_FILE""##,
            "",
            "# Force autoload all functions first",
            "declare -f > /dev/null 2>&1",
            "",
            "# Now get user function names - filter completion functions (single underscore prefix)",
            "# but keep double-underscore helpers (e.g. __zsh_like_cd from mise, __pyenv_init)",
            "declare -F | cut -d' ' -f3 | grep -vE '^_[^_]' | while read func; do",
            "  # Encode the function to base64, preserving all special characters",
            r##"  encoded_func=$(declare -f "$func" | base64 )"##,
            "  # Write the function definition to the snapshot",
            r##"  echo "eval \"\$(echo '$encoded_func' | base64 -d)\" > /dev/null 2>&1" >> "$SNAPSHOT_FILE""##,
            "done",
            "",
            r##"echo "# Shell Options" >> "$SNAPSHOT_FILE""##,
            r##"shopt -p | head -n 1000 >> "$SNAPSHOT_FILE""##,
            // 有意差异：TS 用 `grep "on"` 过滤，会匹配选项名（monitor、onecmd 等处于 off 也被写入），
            // 使每条命令开启作业控制，后台任务脱离命令的进程组而残留。这里只写入真正为 on 的选项。
            TS_SET_O_LINE_FIXED,
            r##"echo "shopt -s expand_aliases" >> "$SNAPSHOT_FILE""##,
        ]
    };
    lines.extend(ALIAS_LINES);
    lines
}

/// TS `applyBashSourceScripts` 的可选 source 行（快照）。
pub fn optional_source_line(quoted_path: &str) -> String {
    format!(". {quoted_path} 2>/dev/null || true")
}

/// TS `createPosixCwdCaptureCommand` / `createWindowsCmdCwdCaptureCommand`。
pub fn cwd_capture(command: &str, cwd_file: &str, cmd: bool) -> String {
    if cmd {
        return [
            command.to_owned(),
            r##"set "__zcode_status=%ERRORLEVEL%""##.to_owned(),
            format!(r##"if "%__zcode_status%"=="0" cd > "{}""##, cwd_file.replace('"', "\"\"")),
            "exit /b %__zcode_status%".to_owned(),
        ]
        .join("\r\n");
    }
    [
        command.to_owned(),
        "__zcode_status=$?".to_owned(),
        format!(r##"if [ "$__zcode_status" -eq 0 ]; then pwd -P > {}; fi"##, quote_always(cwd_file)),
        r##"exit "$__zcode_status""##.to_owned(),
    ]
    .join("\n")
}

/// TS `gitBashPathToWindowsPath`。
pub fn git_bash_to_windows(value: &str) -> String {
    if value.starts_with("//") {
        return value.replace('/', "\\");
    }
    let rest_to_windows = |rest: &str| {
        let converted = rest.replace('/', "\\");
        let converted = converted.trim_start_matches('\\');
        if converted.is_empty() { "\\".to_owned() } else { format!("\\{converted}") }
    };
    let drive = |rest: &str| -> Option<(char, usize)> {
        let mut chars = rest.chars();
        let letter = chars.next().filter(char::is_ascii_alphabetic)?;
        matches!(chars.next(), None | Some('/')).then_some((letter, 1))
    };
    if let Some(rest) = value.strip_prefix("/cygdrive/")
        && let Some((letter, len)) = drive(rest)
    {
        return format!("{}:{}", letter.to_ascii_uppercase(), rest_to_windows(&rest[len..]));
    }
    if let Some(rest) = value.strip_prefix('/')
        && let Some((letter, len)) = drive(rest)
    {
        return format!("{}:{}", letter.to_ascii_uppercase(), rest_to_windows(&rest[len..]));
    }
    value.replace('/', "\\")
}

/// TS `appendBashCwdStderrSuffix`（`Shell cwd was reset to <root>`）。
pub fn reset_suffix(stderr: &str, workspace_root: &str) -> String {
    let suffix = format!("Shell cwd was reset to {workspace_root}");
    let trimmed = stderr.trim_end_matches(['\r', '\n']);
    if trimmed.is_empty() { suffix } else { format!("{trimmed}\n{suffix}") }
}

/// TS `shellQuoteAlways`。
pub fn quote_always(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

/// TS `shellDoubleQuote`。
fn double_quote(value: &str) -> String {
    let escaped = value.replace('\\', r"\\").replace('"', r##"\""##).replace('$', r"\$").replace('`', r"\`");
    format!("\"{escaped}\"")
}

#[cfg(test)]
#[path = "shell_snapshot_tests.rs"]
mod tests;
