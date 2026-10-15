//! terminal 启动参数解析（TS `terminalService.ts` 的 `resolveTerminalShell` / `resolveTerminalCwd` /
//! `resolveTerminalEnv`）。env 一律由调用方传入快照（经 `core_api::env` 唯一出口取得），本模块不读进程 env。

use std::collections::BTreeMap;

pub type Env = [(String, String)];

/// macOS 从 Dock/Finder 启动时父进程 PATH 过窄，终端里补常见 Homebrew 与系统路径（TS `DARWIN_GUI_FALLBACK_PATHS`）。
const DARWIN_GUI_FALLBACK_PATHS: [&str; 8] = [
    "/opt/homebrew/bin",
    "/opt/homebrew/sbin",
    "/usr/local/bin",
    "/usr/local/sbin",
    "/usr/bin",
    "/bin",
    "/usr/sbin",
    "/sbin",
];

#[cfg(windows)]
const PATH_DELIMITER: char = ';';
#[cfg(not(windows))]
const PATH_DELIMITER: char = ':';

pub fn var<'a>(env: &'a Env, name: &str) -> Option<&'a str> {
    env.iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

/// JS `/utf-?8/i.test(value)`。
fn is_utf8_locale(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    lower.contains("utf8") || lower.contains("utf-8")
}

/// TS `isMissingOrCLocale`：trim 后大写为 `""` / `C` / `POSIX`。
fn is_missing_or_c_locale(value: Option<&str>) -> bool {
    let normalized = value.unwrap_or_default().trim().to_uppercase();
    normalized.is_empty() || normalized == "C" || normalized == "POSIX"
}

/// TS `mergePathEntries`：按分隔符拆分、trim、去空、去重，保序。
pub fn merge_path_entries(entries: &[Option<&str>]) -> String {
    let mut merged: Vec<String> = Vec::new();
    for value in entries.iter().flatten() {
        for entry in value.split(PATH_DELIMITER) {
            let trimmed = entry.trim();
            if !trimmed.is_empty() && !merged.iter().any(|seen| seen == trimmed) {
                merged.push(trimmed.to_owned());
            }
        }
    }
    merged.join(&PATH_DELIMITER.to_string())
}

/// TS `resolveFallbackUtf8Locale`。
fn fallback_utf8_locale(env: &Env, darwin: bool) -> String {
    ["LC_ALL", "LC_CTYPE", "LANG"]
        .iter()
        .filter_map(|name| var(env, name))
        .find(|value| is_utf8_locale(value))
        .map_or_else(
            || if darwin { "en_US.UTF-8" } else { "C.UTF-8" }.to_owned(),
            str::to_owned,
        )
}

/// TS `resolveTerminalEnv`。`darwin` 作为参数而非 `cfg!`，让规则可在任意平台单测。
pub fn terminal_env(env: &Env, darwin: bool) -> BTreeMap<String, String> {
    let mut next: BTreeMap<String, String> = env.iter().cloned().collect();
    let fallback = fallback_utf8_locale(env, darwin);
    if darwin {
        let extra: Vec<Option<&str>> = std::iter::once(var(env, "PATH"))
            .chain(DARWIN_GUI_FALLBACK_PATHS.iter().map(|path| Some(*path)))
            .collect();
        next.insert("PATH".to_owned(), merge_path_entries(&extra));
    }
    // 真实 terminal panel 必须作为交互终端启动（runtime 采集登录 shell env 时用的 TERM=dumb/CI=1 不能带进来）。
    next.insert("TERM".to_owned(), "xterm-256color".to_owned());
    let colorterm = next
        .get("COLORTERM")
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "truecolor".to_owned());
    next.insert("COLORTERM".to_owned(), colorterm);
    if next.get("CI").map(String::as_str) == Some("1") && var(env, "TERM") == Some("dumb") {
        next.remove("CI");
    }
    // 只在 locale 缺失或明确为 C/POSIX 时补 UTF-8，保留用户已配置好的 locale。
    for name in ["LANG", "LC_CTYPE"] {
        if is_missing_or_c_locale(next.get(name).map(String::as_str)) {
            next.insert(name.to_owned(), fallback.clone());
        }
    }
    if next.contains_key("LC_ALL") && is_missing_or_c_locale(next.get("LC_ALL").map(String::as_str))
    {
        next.insert("LC_ALL".to_owned(), fallback);
    }
    next
}

/// TS `isExecutable`：含路径分隔符的直接检查，否则在 PATH 里逐目录找。
async fn is_executable(command: &str, env: &Env) -> bool {
    if command.contains(['/', '\\']) {
        return executable_file(command).await;
    }
    let Some(path) = var(env, "PATH").filter(|path| !path.is_empty()) else {
        return false;
    };
    for dir in path.split(PATH_DELIMITER).filter(|dir| !dir.is_empty()) {
        if executable_file(&format!("{dir}/{command}")).await {
            return true;
        }
    }
    false
}

/// 跟随符号链接；Unix 要求任一执行位，Windows 只要求存在（与 `accessSync(X_OK)` 的平台语义一致）。
async fn executable_file(path: &str) -> bool {
    let Ok(info) = tokio::fs::metadata(path).await else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        info.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        let _ = info;
        true
    }
}

/// TS `resolveTerminalShell`。
pub async fn terminal_shell(env: &Env, windows: bool) -> anyhow::Result<String> {
    if windows {
        let comspec = var(env, "ComSpec");
        // PowerShell 7+ 的 ConPTY 兼容性更好，优先 pwsh；找不到再回退系统自带 shell。
        for candidate in [
            Some("pwsh.exe"),
            Some("powershell.exe"),
            comspec,
            Some("cmd.exe"),
        ]
        .into_iter()
        .flatten()
        {
            if !candidate.is_empty() && is_executable(candidate, env).await {
                return Ok(candidate.to_owned());
            }
        }
        anyhow::bail!("No usable Windows shell found for terminal startup");
    }
    // SHELL 可能残留一个不存在的路径：先校验可执行，再按常见顺序回退。
    for candidate in [
        var(env, "SHELL"),
        Some("/bin/zsh"),
        Some("/bin/bash"),
        Some("/bin/sh"),
    ]
    .into_iter()
    .flatten()
    {
        if !candidate.is_empty() && is_executable(candidate, env).await {
            return Ok(candidate.to_owned());
        }
    }
    anyhow::bail!("No usable shell found for terminal startup");
}

/// TS `resolveTerminalCwd`：传入目录失效（被删除/移动）时回退 HOME、根目录，保证终端还能拉起。
pub async fn terminal_cwd(env: &Env, requested: Option<&str>) -> anyhow::Result<String> {
    for candidate in [requested, var(env, "HOME"), Some("/")]
        .into_iter()
        .flatten()
    {
        if candidate.is_empty() {
            continue;
        }
        if tokio::fs::metadata(candidate)
            .await
            .is_ok_and(|info| info.is_dir())
        {
            return Ok(candidate.to_owned());
        }
    }
    anyhow::bail!("No usable working directory found for terminal startup");
}

#[cfg(test)]
#[path = "launch_tests.rs"]
mod tests;
