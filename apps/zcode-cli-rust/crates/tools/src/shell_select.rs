//! Shell 选择：与 TS `adapters/src/exec/bash-shell-provider.ts` 的
//! `resolveEffectiveBashShellSelection` 一一对应，见 docs/specs/rust-shell-selection.md。
//! 纯函数：平台、环境和可执行检查全部由调用方传入，便于跨平台差分测试。

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Windows,
    Posix,
}

impl Platform {
    pub fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else {
            Self::Posix
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dialect {
    GitBash,
    Posix,
    Cmd,
    Legacy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    AutoDetected,
    UserConfig,
    LegacyFallback,
}

/// Host 下发的用户选择（`IntegratedTerminalShellSelection`，`mode=auto` 时不传）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Override {
    pub dialect: Dialect,
    pub path: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selection {
    pub dialect: Dialect,
    pub path: Option<String>,
    pub source: Source,
}

/// 实际 spawn 参数；`envOverlay` 与 TS provider 相同。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpawnPlan {
    pub file: String,
    pub args: Vec<String>,
    pub env_overlay: Vec<(String, String)>,
}

const FIXED_POSIX_SHELL_DIRS: [&str; 4] =
    ["/bin", "/usr/bin", "/usr/local/bin", "/opt/homebrew/bin"];
const WINDOWS_GIT_BASH_PATHS: [&str; 2] = [
    "C:\\Program Files\\Git\\bin\\bash.exe",
    "C:\\Program Files (x86)\\Git\\bin\\bash.exe",
];
const DEFAULT_WINDOWS_PATHEXT: [&str; 4] = [".COM", ".EXE", ".BAT", ".CMD"];

/// 解析 Host `session/requestRuntimePreferences` 结果中的 `integratedTerminalShell`；
/// 与 TS `integratedTerminalShellToExecutionSelection` 一致，`mode=auto` 或非法值视为无覆盖。
pub fn parse_override(value: &serde_json::Value) -> Option<Override> {
    if value["mode"] != "shell" {
        return None;
    }
    let dialect = match value["dialect"].as_str()? {
        "cmd" => Dialect::Cmd,
        "git-bash" => Dialect::GitBash,
        _ => return None,
    };
    let path = value["path"].as_str().filter(|p| !p.is_empty())?;
    Some(Override {
        dialect,
        path: Some(path.into()),
    })
}

pub fn resolve(
    platform: Platform,
    env: &[(String, String)],
    over: Option<&Override>,
    exists: &dyn Fn(&str) -> bool,
) -> Selection {
    match platform {
        Platform::Windows => resolve_windows(env, over, exists),
        Platform::Posix => resolve_posix(env, exists),
    }
}

/// 把选择结果转成 spawn 参数。git-bash/posix 与 TS 一样用登录 shell `-c -l`；
/// cmd 与 legacy 在 Windows 走 `ComSpec /d /s /c`（等价 Node `shell: string`），POSIX legacy 走 `/bin/sh -c`。
pub fn spawn_plan(
    platform: Platform,
    env: &[(String, String)],
    selection: &Selection,
    command: &str,
) -> SpawnPlan {
    match (selection.dialect, &selection.path) {
        (Dialect::GitBash | Dialect::Posix, Some(path)) => SpawnPlan {
            file: path.clone(),
            args: vec!["-c".into(), "-l".into(), command.into()],
            env_overlay: vec![
                ("GIT_EDITOR".into(), "true".into()),
                ("SHELL".into(), path.clone()),
            ],
        },
        (Dialect::Cmd, Some(path)) => cmd_plan(path, command),
        _ if platform == Platform::Windows => {
            let comspec = env_value(env, "ComSpec", true).unwrap_or_else(|| "cmd.exe".into());
            cmd_plan(&comspec, command)
        }
        _ => SpawnPlan {
            file: "/bin/sh".into(),
            args: vec!["-c".into(), command.into()],
            env_overlay: Vec::new(),
        },
    }
}

fn cmd_plan(path: &str, command: &str) -> SpawnPlan {
    SpawnPlan {
        file: path.into(),
        args: vec!["/d".into(), "/s".into(), "/c".into(), command.into()],
        env_overlay: Vec::new(),
    }
}

fn resolve_windows(
    env: &[(String, String)],
    over: Option<&Override>,
    exists: &dyn Fn(&str) -> bool,
) -> Selection {
    if let Some(over) = over {
        match (over.dialect, &over.path) {
            (Dialect::GitBash, Some(path)) if exists(path) => {
                return selection(Dialect::GitBash, path, Source::UserConfig);
            }
            (Dialect::Cmd, path) => {
                let path = path.clone().unwrap_or_else(|| "cmd.exe".into());
                // 与 TS 一致：裸 `cmd.exe` 不做可执行预校验，否则用户显式选择会被 Git Bash 抢走。
                if exists(&path) || is_cmd_fallback(&path) {
                    return selection(Dialect::Cmd, &path, Source::UserConfig);
                }
            }
            _ => {}
        }
    }
    if let Some(path) = windows_git_bash(env, exists) {
        return selection(Dialect::GitBash, &path, Source::AutoDetected);
    }
    legacy()
}

fn windows_git_bash(env: &[(String, String)], exists: &dyn Fn(&str) -> bool) -> Option<String> {
    if let Some(path) = WINDOWS_GIT_BASH_PATHS.iter().find(|p| exists(p)) {
        return Some((*path).into());
    }
    let git = windows_executable_candidates("git", env)
        .into_iter()
        .find(|c| exists(c))?;
    let dir = win_dirname(&git);
    [
        win_normalize(&format!("{dir}\\..\\bin\\bash.exe")),
        win_normalize(&format!("{dir}\\..\\..\\bin\\bash.exe")),
    ]
    .into_iter()
    .find(|c| exists(c))
}

fn windows_executable_candidates(file: &str, env: &[(String, String)]) -> Vec<String> {
    let exts: Vec<String> = env_value(env, "PATHEXT", true)
        .map(|raw| {
            raw.split(';')
                .map(str::trim)
                .filter(|e| !e.is_empty())
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| {
            DEFAULT_WINDOWS_PATHEXT
                .iter()
                .map(|e| (*e).into())
                .collect()
        });
    let mut names = vec![file.to_owned()];
    names.extend(exts.iter().map(|e| format!("{file}{}", e.to_lowercase())));
    env_value(env, "PATH", true)
        .unwrap_or_default()
        .split(';')
        .filter(|d| !d.is_empty())
        .flat_map(|dir| {
            names
                .iter()
                .map(move |n| win_normalize(&format!("{dir}\\{n}")))
        })
        .collect()
}

fn resolve_posix(env: &[(String, String)], exists: &dyn Fn(&str) -> bool) -> Selection {
    let shell = env_value(env, "SHELL", false);
    let mut candidates = Vec::new();
    if let Some(shell) = shell.as_ref().filter(|s| posix_kind(s).is_some()) {
        candidates.push(shell.clone());
    }
    let kinds = if shell.as_deref().and_then(posix_kind) == Some("bash") {
        ["bash", "zsh"]
    } else {
        ["zsh", "bash"]
    };
    let path = env_value(env, "PATH", false).unwrap_or_default();
    for kind in kinds {
        for dir in path.split(':').filter(|d| !d.is_empty()) {
            candidates.push(posix_join(dir, kind));
        }
        for dir in FIXED_POSIX_SHELL_DIRS {
            candidates.push(posix_join(dir, kind));
        }
    }
    let mut seen = std::collections::HashSet::new();
    candidates
        .into_iter()
        .filter(|c| seen.insert(c.clone()))
        .find(|c| exists(c))
        .map(|p| selection(Dialect::Posix, &p, Source::AutoDetected))
        .unwrap_or_else(legacy)
}

fn posix_kind(path: &str) -> Option<&'static str> {
    let name = path.rsplit('/').next().unwrap_or(path);
    if name.contains("bash") {
        Some("bash")
    } else if name.contains("zsh") {
        Some("zsh")
    } else {
        None
    }
}

fn posix_join(dir: &str, name: &str) -> String {
    format!("{}/{name}", dir.trim_end_matches('/'))
}

fn is_cmd_fallback(path: &str) -> bool {
    !path.contains('\\') && !path.contains('/') && path.eq_ignore_ascii_case("cmd.exe")
}

fn selection(dialect: Dialect, path: &str, source: Source) -> Selection {
    Selection {
        dialect,
        path: Some(path.into()),
        source,
    }
}

fn legacy() -> Selection {
    Selection {
        dialect: Dialect::Legacy,
        path: None,
        source: Source::LegacyFallback,
    }
}

/// Windows 环境变量大小写不敏感（对应 TS `getWindowsEnvValue`）。
fn env_value(env: &[(String, String)], key: &str, windows: bool) -> Option<String> {
    env.iter()
        .find(|(k, _)| {
            if windows {
                k.eq_ignore_ascii_case(key)
            } else {
                k == key
            }
        })
        .map(|(_, v)| v.clone())
}

fn win_dirname(path: &str) -> String {
    match path.rfind(['\\', '/']) {
        Some(i) => path[..i].to_owned(),
        None => ".".into(),
    }
}

/// `path.win32.normalize` 的子集：统一分隔符、折叠 `.`/`..`/重复分隔符。
fn win_normalize(path: &str) -> String {
    let (prefix, rest) = match path.get(1..2) {
        Some(":") => path.split_at(2),
        _ => ("", path),
    };
    let absolute = rest.starts_with(['\\', '/']);
    let mut parts: Vec<&str> = Vec::new();
    for seg in rest.split(['\\', '/']) {
        match seg {
            "" | "." => {}
            ".." if parts.last().is_some_and(|p| *p != "..") => {
                parts.pop();
            }
            ".." if absolute => {}
            _ => parts.push(seg),
        }
    }
    let body = parts.join("\\");
    if absolute {
        format!("{prefix}\\{body}")
    } else {
        format!("{prefix}{body}")
    }
}

#[cfg(test)]
#[path = "shell_select_tests.rs"]
mod tests;
