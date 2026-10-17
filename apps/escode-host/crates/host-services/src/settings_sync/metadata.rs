//! skill / command / plugin 的元数据读取（TS `readSkillMetadata` / `readCommandMetadata` / `readPluginMetadata`）。

use std::path::{Path, PathBuf};

use saphyr::{LoadableYamlNode, ScalarOwned, YamlOwned};
use serde_json::Value;

use crate::commands::parser;
use crate::git::paths::js_trim;

pub struct SkillMetadata {
    pub name: String,
    pub version: Option<String>,
}

/// TS `readSkillMetadata`：读取失败回退目录名。
pub fn read_skill_metadata(skill_path: &Path) -> SkillMetadata {
    let fallback = skill_path
        .parent()
        .and_then(Path::file_name)
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let Ok(content) = std::fs::read_to_string(skill_path) else {
        return SkillMetadata {
            name: fallback,
            version: None,
        };
    };
    read_skill_metadata_from_markdown(&content, &fallback)
}

fn read_skill_metadata_from_markdown(content: &str, fallback_name: &str) -> SkillMetadata {
    let normalized = content.replace("\r\n", "\n").replace('\r', "\n");
    let Some(frontmatter) = split_frontmatter(&normalized) else {
        return SkillMetadata {
            name: fallback_name.to_owned(),
            version: None,
        };
    };
    let parsed = YamlOwned::load_from_str(frontmatter).ok();
    if let Some(documents) = parsed
        && documents.len() == 1
        && let YamlOwned::Mapping(map) = &documents[0]
    {
        let (mut name, mut version) = (None, None);
        for (key, value) in map {
            let YamlOwned::Value(ScalarOwned::String(key)) = key else {
                continue;
            };
            match key.as_str() {
                "name" => {
                    if let YamlOwned::Value(ScalarOwned::String(text)) = value {
                        let text = js_trim(text);
                        if !text.is_empty() {
                            name = Some(text.to_owned());
                        }
                    }
                }
                "version" => match value {
                    YamlOwned::Value(ScalarOwned::String(text)) => {
                        let text = js_trim(text);
                        if !text.is_empty() {
                            version = Some(text.to_owned());
                        }
                    }
                    YamlOwned::Value(ScalarOwned::Integer(number)) => {
                        version = Some(number.to_string());
                    }
                    YamlOwned::Value(ScalarOwned::FloatingPoint(number)) => {
                        version = Some(number.0.to_string());
                    }
                    _ => {}
                },
                _ => {}
            }
        }
        return SkillMetadata {
            name: name.unwrap_or_else(|| loose_name(frontmatter, fallback_name)),
            version: version.or_else(|| loose_version(frontmatter)),
        };
    }
    SkillMetadata {
        name: loose_name(frontmatter, fallback_name),
        version: loose_version(frontmatter),
    }
}

fn split_frontmatter(content: &str) -> Option<&str> {
    let rest = content.strip_prefix("---\n")?;
    let mut from = 0;
    while let Some(found) = rest[from..].find("\n---") {
        let at = from + found;
        match rest.as_bytes().get(at + 4) {
            None | Some(b'\n') => return Some(&rest[..at]),
            _ => from = at + 1,
        }
    }
    None
}

/// TS `readLooseSkillName`：`^name\s*:\s*(.+)$` 的首个行首匹配。
fn loose_name(frontmatter: &str, fallback: &str) -> String {
    let value = loose_field(frontmatter, "name").unwrap_or_default();
    if value.is_empty() {
        fallback.to_owned()
    } else {
        value
    }
}

fn loose_version(frontmatter: &str) -> Option<String> {
    loose_field(frontmatter, "version").filter(|version| !version.is_empty())
}

fn loose_field(frontmatter: &str, key: &str) -> Option<String> {
    for line in frontmatter.split('\n') {
        let Some(rest) = line.strip_prefix(key) else {
            continue;
        };
        let rest = rest.trim_start_matches(is_js_space);
        let Some(rest) = rest.strip_prefix(':') else {
            continue;
        };
        let value = rest.trim_start_matches(is_js_space);
        if value.is_empty() {
            continue;
        }
        return Some(strip_yaml_scalar_quotes(value));
    }
    None
}

fn is_js_space(character: char) -> bool {
    (character.is_whitespace() && character != '\u{85}') || character == '\u{feff}'
}

fn strip_yaml_scalar_quotes(value: &str) -> String {
    let trimmed = js_trim(value);
    if trimmed.len() >= 2
        && ((trimmed.starts_with('"') && trimmed.ends_with('"'))
            || (trimmed.starts_with('\'') && trimmed.ends_with('\'')))
    {
        return js_trim(&trimmed[1..trimmed.len() - 1]).to_owned();
    }
    trimmed.to_owned()
}

pub struct CommandMetadata {
    pub name: String,
    pub description: Option<String>,
    pub argument_hint: Option<String>,
}

/// TS `readCommandMetadata` + `getCommandNameFromPath`：名称来自**相对路径**，不是文件名。
pub fn read_command_metadata(file_path: &Path, root: &Path) -> CommandMetadata {
    let relative = file_path
        .strip_prefix(root)
        .map(relative_string)
        .unwrap_or_default();
    let without_ext =
        if relative.len() >= 3 && relative[relative.len() - 3..].eq_ignore_ascii_case(".md") {
            &relative[..relative.len() - 3]
        } else {
            &relative
        };
    let name = format!("/{without_ext}");
    let Ok(content) = std::fs::read_to_string(file_path) else {
        return CommandMetadata {
            name,
            description: None,
            argument_hint: None,
        };
    };
    match parser::parse_command_file(&content, &file_path.to_string_lossy()) {
        Some(parsed) => CommandMetadata {
            name,
            description: parsed.description.filter(|text| !text.is_empty()),
            argument_hint: parsed.argument_hint.filter(|text| !text.is_empty()),
        },
        None => CommandMetadata {
            name,
            description: None,
            argument_hint: None,
        },
    }
}

pub fn relative_string(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

pub struct PluginMetadata {
    pub id: String,
    pub name: String,
    pub version: Option<String>,
}

/// TS `findPluginManifestPath`：四个 manifest 目录按顺序探测（`lstat` 存在即算）。
pub fn find_plugin_manifest(plugin_path: &Path) -> Option<PathBuf> {
    for segments in [
        [".escode-plugin", "plugin.json"],
        [".zcode-plugin", "plugin.json"],
        [".claude-plugin", "plugin.json"],
        [".codex-plugin", "plugin.json"],
    ] {
        let path = plugin_path.join(segments[0]).join(segments[1]);
        if std::fs::symlink_metadata(&path).is_ok() {
            return Some(path);
        }
    }
    None
}

/// TS `readPluginMetadata`：manifest 不是对象/没有非空 name → None。
pub fn read_plugin_metadata(plugin_path: &Path) -> Option<PluginMetadata> {
    let manifest = find_plugin_manifest(plugin_path)?;
    let raw = std::fs::read_to_string(manifest).ok()?;
    let parsed: Value = serde_json::from_str(&raw).ok()?;
    let object = parsed.as_object()?;
    let name = object
        .get("name")
        .and_then(Value::as_str)
        .map(js_trim)
        .filter(|name| !name.is_empty())?
        .to_owned();
    let version = object
        .get("version")
        .and_then(Value::as_str)
        .map(js_trim)
        .filter(|version| !version.is_empty())
        .map(str::to_owned);
    Some(PluginMetadata {
        id: format!("{name}@inline"),
        name,
        version,
    })
}

#[cfg(test)]
#[path = "metadata_tests.rs"]
mod tests;
