//! 终端字体 / 主题 profile（TS `terminalProfile.ts`）。检测器顺序与优先级照搬；Alacritty 与 macOS 检测器见
//! docs/specs/rust-u2-terminal.md §6.1（T2 切片）。

use std::path::Path;

use serde_json::{Map, Value};

use super::jsonc::{is_js_space, parse_jsonc};
use super::launch::{Env, var};
use crate::git::paths::js_trim;

/// TS `FONT_FAMILY_FALLBACKS`。
const FONT_FAMILY_FALLBACKS: [&str; 12] = [
    "ui-monospace",
    "SFMono-Regular",
    "SF Mono",
    "Menlo",
    "Monaco",
    "Consolas",
    "Cascadia Mono",
    "JetBrains Mono",
    "MesloLGS NF",
    "Hack Nerd Font",
    "Noto Sans Mono CJK SC",
    "monospace",
];

/// Node `process.platform` 口径的平台名（检测器按它过滤）。
pub fn platform() -> &'static str {
    if cfg!(target_os = "windows") {
        "win32"
    } else if cfg!(target_os = "macos") {
        "darwin"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else if cfg!(target_os = "freebsd") {
        "freebsd"
    } else if cfg!(target_os = "openbsd") {
        "openbsd"
    } else {
        "other"
    }
}

/// TS `TerminalDetectedProfile`。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Detected {
    pub font_family: Option<String>,
    pub font_size: Option<f64>,
    pub theme: Option<Value>,
}

impl Detected {
    fn font(family: String) -> Self {
        Self {
            font_family: Some(family),
            ..Self::default()
        }
    }
}

/// TS `TerminalFontProfile`。
#[derive(Clone, Debug, PartialEq)]
pub struct FontProfile {
    pub font_family: String,
    pub font_size: Option<f64>,
    pub theme: Option<Value>,
    pub source: &'static str,
}

fn is_line_terminator(character: char) -> bool {
    matches!(character, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

fn normalize_font_family(value: Option<&str>) -> Option<String> {
    let trimmed = js_trim(value?);
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

/// TS `dedupeFontFamilyStack`。
fn dedupe_font_family_stack(primary: &str) -> String {
    let mut stack: Vec<String> = primary
        .split(',')
        .map(|part| js_trim(part).to_owned())
        .filter(|part| !part.is_empty())
        .collect();
    for fallback in FONT_FAMILY_FALLBACKS {
        if !stack.iter().any(|item| item == fallback) {
            stack.push(fallback.to_owned());
        }
    }
    stack.join(", ")
}

async fn read_jsonc_file(path: &Path) -> Option<Map<String, Value>> {
    parse_jsonc(&tokio::fs::read_to_string(path).await.ok()?)
}

/// TS `readNestedString`。
fn nested_string(value: &Value, segments: &[&str]) -> Option<String> {
    let mut current = value;
    for segment in segments {
        current = current.as_object()?.get(*segment)?;
    }
    normalize_font_family(current.as_str())
}

fn home_dir(env: &Env) -> String {
    ["HOME", "USERPROFILE"]
        .iter()
        .filter_map(|name| var(env, name))
        .map(js_trim)
        .find(|value| !value.is_empty())
        .unwrap_or_default()
        .to_owned()
}

fn trimmed_var(env: &Env, name: &str) -> Option<String> {
    var(env, name)
        .map(js_trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

/// TS `detectWindowsTerminalFontFamily`。
async fn detect_windows_terminal(env: &Env) -> Option<Detected> {
    let base = trimmed_var(env, "LOCALAPPDATA").or_else(|| trimmed_var(env, "APPDATA"))?;
    let base = Path::new(&base);
    let candidates = [
        base.join("Packages/Microsoft.WindowsTerminal_8wekyb3d8bbwe/LocalState/settings.json"),
        base.join(
            "Packages/Microsoft.WindowsTerminalPreview_8wekyb3d8bbwe/LocalState/settings.json",
        ),
        base.join("Microsoft/Windows Terminal/settings.json"),
    ];
    for path in candidates {
        let Some(parsed) = read_jsonc_file(&path).await else {
            continue;
        };
        let parsed = Value::Object(parsed);
        let profiles = parsed.get("profiles");
        let list: &[Value] = profiles
            .and_then(|profiles| profiles.get("list"))
            .and_then(Value::as_array)
            .map_or(&[], Vec::as_slice);
        if let Some(default_id) = nested_string(&parsed, &["defaultProfile"]) {
            for profile in list {
                if profile.get("guid").and_then(Value::as_str) != Some(default_id.as_str()) {
                    continue;
                }
                if let Some(face) = nested_string(profile, &["font", "face"]) {
                    return Some(Detected::font(face));
                }
            }
        }
        if let Some(face) = profiles
            .and_then(|profiles| profiles.get("defaults"))
            .and_then(|defaults| nested_string(defaults, &["font", "face"]))
        {
            return Some(Detected::font(face));
        }
        for profile in list {
            if let Some(face) = nested_string(profile, &["font", "face"]) {
                return Some(Detected::font(face));
            }
        }
    }
    None
}

/// TS `detectVsCodeTerminalFontFamily`。
async fn detect_vscode(env: &Env) -> Option<Detected> {
    let home = home_dir(env);
    let mut bases: Vec<String> = Vec::new();
    for root in [
        trimmed_var(env, "APPDATA"),
        trimmed_var(env, "XDG_CONFIG_HOME"),
    ]
    .into_iter()
    .flatten()
    {
        bases.push(format!("{root}/Code/User/settings.json"));
        bases.push(format!("{root}/Code - Insiders/User/settings.json"));
    }
    for relative in [".config", "Library/Application Support"] {
        bases.push(format!("{home}/{relative}/Code/User/settings.json"));
        bases.push(format!(
            "{home}/{relative}/Code - Insiders/User/settings.json"
        ));
    }
    for path in bases {
        let Some(parsed) = read_jsonc_file(Path::new(&path)).await else {
            continue;
        };
        if let Some(family) =
            nested_string(&Value::Object(parsed), &["terminal.integrated.fontFamily"])
        {
            return Some(Detected::font(family));
        }
    }
    None
}

/// JS `raw.match(/^\s*font_family\s+(.+)$/m)?.[1]`（含 `\s` 跨行与回溯语义）。
pub fn kitty_font_family_match(raw: &str) -> Option<String> {
    let chars: Vec<char> = raw.chars().collect();
    let line_starts = std::iter::once(0).chain(
        chars
            .iter()
            .enumerate()
            .filter(|(_, c)| **c == '\n')
            .map(|(index, _)| index + 1),
    );
    for start in line_starts {
        let mut position = start;
        while position < chars.len() && is_js_space(chars[position]) {
            position += 1;
        }
        let keyword: Vec<char> = "font_family".chars().collect();
        if chars.get(position..position + keyword.len()) != Some(keyword.as_slice()) {
            continue;
        }
        let space_start = position + keyword.len();
        let mut space_end = space_start;
        while space_end < chars.len() && is_js_space(chars[space_end]) {
            space_end += 1;
        }
        // `\s+` 贪婪后 `.+` 至少要一个非行终止符；不足时逐字符回溯。
        for group_start in (space_start + 1..=space_end).rev() {
            if group_start < chars.len() && !is_line_terminator(chars[group_start]) {
                let group_end = chars[group_start..]
                    .iter()
                    .position(|c| is_line_terminator(*c))
                    .map_or(chars.len(), |offset| group_start + offset);
                return Some(chars[group_start..group_end].iter().collect());
            }
        }
    }
    None
}

/// TS `detectKittyFontFamily`。
async fn detect_kitty(env: &Env) -> Option<Detected> {
    let home = home_dir(env);
    let xdg = trimmed_var(env, "XDG_CONFIG_HOME").unwrap_or_else(|| format!("{home}/.config"));
    for path in [
        format!("{xdg}/kitty/kitty.conf"),
        format!("{home}/Library/Application Support/kitty/kitty.conf"),
    ] {
        let Ok(raw) = tokio::fs::read_to_string(&path).await else {
            continue;
        };
        if let Some(family) = normalize_font_family(kitty_font_family_match(&raw).as_deref()) {
            let family = family.strip_prefix('"').unwrap_or(&family);
            let family = family.strip_suffix('"').unwrap_or(family);
            return Some(Detected::font(family.to_owned()));
        }
    }
    None
}

/// TS `detectSystemTerminalProfile`：检测器顺序即优先级，首个有内容的结果胜出。
pub async fn detect_system_profile(env: &Env, platform: &str) -> Option<Detected> {
    let unix_like = ["darwin", "linux", "freebsd", "openbsd"].contains(&platform);
    let mut candidates: Vec<Option<Detected>> = Vec::new();
    if platform == "win32" {
        candidates.push(detect_windows_terminal(env).await);
    }
    if candidates.iter().flatten().all(is_empty_detected) {
        candidates.push(detect_vscode(env).await);
    }
    if candidates.iter().flatten().all(is_empty_detected) && unix_like {
        candidates.push(detect_kitty(env).await);
    }
    candidates
        .into_iter()
        .flatten()
        .find(|detected| !is_empty_detected(detected))
}

fn is_empty_detected(detected: &Detected) -> bool {
    detected.font_family.as_deref().is_none_or(str::is_empty)
        && detected.font_size.is_none_or(|size| size == 0.0)
        && detected.theme.is_none()
}

/// TS `resolveTerminalFontProfile`。
pub async fn resolve_font_profile(
    custom_font_family: Option<&str>,
    inherit_system_profile: bool,
    env: &Env,
    platform: &str,
) -> FontProfile {
    let custom = normalize_font_family(custom_font_family);
    let detected = if inherit_system_profile {
        detect_system_profile(env, platform).await
    } else {
        None
    };
    if let Some(custom) = custom {
        return FontProfile {
            font_family: dedupe_font_family_stack(&custom),
            font_size: detected.as_ref().and_then(|d| d.font_size),
            theme: detected.and_then(|d| d.theme),
            source: "custom",
        };
    }
    if let Some(detected) = detected {
        return FontProfile {
            // 读不到字体族时不能整体放弃系统 profile：仅配置配色的终端也要继承字号与配色。
            font_family: dedupe_font_family_stack(
                detected
                    .font_family
                    .as_deref()
                    .filter(|f| !f.is_empty())
                    .unwrap_or(FONT_FAMILY_FALLBACKS[0]),
            ),
            font_size: detected.font_size,
            theme: detected.theme,
            source: "system",
        };
    }
    FontProfile {
        font_family: FONT_FAMILY_FALLBACKS.join(", "),
        font_size: None,
        theme: None,
        source: "fallback",
    }
}

#[cfg(test)]
#[path = "profile_tests.rs"]
mod tests;
