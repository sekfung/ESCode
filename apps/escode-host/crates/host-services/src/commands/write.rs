//! `commands` 的写面：`writeCommandFile` / `updateCommandFile` / `deleteCommandFile` /
//! `setCommandEnabled`（TS `commandsService.ts` 的同名方法）。
//!
//! 三条语义要点（逐条对照 TS）：
//! 1. **新建即启用**：写盘后把该文件的启用覆盖**删掉**（启用态不落盘）；
//! 2. **改名迁覆盖**：编辑命令换了文件名时，旧的 `enable` 覆盖要迁到新路径，否则用户刚禁用的命令
//!    会因改名重新启用；
//! 3. 删除 / 禁用都把覆盖表收敛回「最简」形态（启用即删键，`command` 空了连键删）。

use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use super::config::{enabled_overrides, is_enabled, read_config, set_override, write_config};
use super::descriptors::{SourceDescriptor, descriptor_for};
use super::parser::{
    CommandConfig as FileConfig, generate_command_file_content, parse_command_file, parsed_fields,
};

pub struct CommandsWriteService {
    home: PathBuf,
}

impl CommandsWriteService {
    pub fn new(home: PathBuf) -> Self {
        Self { home }
    }

    /// TS `getCommandsRootForStorage`：`project` 需要 `workspacePath`，缺失即报错。
    fn root_for_storage(
        &self,
        descriptor: &SourceDescriptor,
        storage_level: Option<&str>,
        workspace_path: Option<&str>,
    ) -> Result<(PathBuf, &'static str, Option<String>), String> {
        if storage_level == Some("project") {
            let workspace = workspace_path
                .filter(|value| !value.is_empty())
                .ok_or_else(|| "Missing workspace path for project command".to_owned())?;
            let mut root = PathBuf::from(workspace);
            for segment in descriptor.workspace_directory_segments {
                root.push(segment);
            }
            return Ok((root, "project", Some(workspace.to_owned())));
        }
        Ok((descriptor.user_directory(&self.home), "global", None))
    }

    pub async fn write_command_file(&self, params: &Value) -> Result<Value, String> {
        let descriptor = descriptor_for(params["agentSource"].as_str())?;
        let config = file_config(&params["config"])?;
        let (root, scope, project_path) = self.root_for_storage(
            descriptor,
            params["storageLevel"].as_str(),
            params["workspacePath"].as_str(),
        )?;
        tokio::fs::create_dir_all(&root)
            .await
            .map_err(|error| format!("{error}"))?;

        let file_name = command_file_name(&config, descriptor)?;
        let file_path = root.join(&file_name);
        if tokio::fs::try_exists(&file_path)
            .await
            .map_err(|error| format!("{error}"))?
        {
            return Err(format!("Command file already exists: {file_name}"));
        }

        let content = generate_command_file_content(&writable(&config, descriptor), None);
        write_file(&file_path, &content).await?;
        let path_text = path_string(&file_path);
        // 新建即启用：把任何既有覆盖删掉。
        let config_json = read_config(&self.home).await;
        write_config(&self.home, &set_override(&config_json, &path_text, true)).await?;

        let parsed = parse_command_file(&content, &path_text)
            .ok_or_else(|| "Failed to parse written command file".to_owned())?;
        let name = command_name(&root, &file_path, descriptor);
        Ok(json!({
            "command": user_command(
                &parsed,
                &name,
                descriptor,
                &root,
                scope,
                project_path.as_deref(),
                true,
            ),
        }))
    }

    pub async fn update_command_file(&self, params: &Value) -> Result<Value, String> {
        let descriptor = descriptor_for(params["agentSource"].as_str())?;
        let config = file_config(&params["config"])?;
        let (root, scope, project_path) = self.root_for_storage(
            descriptor,
            params["storageLevel"].as_str(),
            params["workspacePath"].as_str(),
        )?;
        let file_name = command_file_name(&config, descriptor)?;
        let new_path = root.join(&file_name);
        let new_text = path_string(&new_path);
        let old_path = params["oldFilePath"]
            .as_str()
            .map(str::to_owned)
            .filter(|value| !value.is_empty());
        let overrides = enabled_overrides(&read_config(&self.home).await);
        let existing = match &old_path {
            Some(old) => tokio::fs::read_to_string(old).await.ok(),
            None => None,
        };

        // 改名：先删旧文件（不存在属正常），再按新名写。
        let renamed = old_path.as_deref().is_some_and(|old| old != new_text);
        if let Some(old) = old_path.as_deref().filter(|_| renamed) {
            let _ = tokio::fs::remove_file(old).await;
            if tokio::fs::try_exists(&new_path)
                .await
                .map_err(|error| format!("{error}"))?
            {
                return Err(format!("Command file already exists: {file_name}"));
            }
        }

        let content =
            generate_command_file_content(&writable(&config, descriptor), existing.as_deref());
        if let Some(parent) = new_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|error| format!("{error}"))?;
        }
        write_file(&new_path, &content).await?;

        // 改名必须把旧的启用覆盖迁到新路径，否则刚禁用的命令会因改名重新启用。
        if renamed
            && let Some(old) = old_path.as_deref()
            && overrides.contains_key(old)
        {
            let migrated = set_override(
                &set_override(&read_config(&self.home).await, old, true),
                &new_text,
                is_enabled(&overrides, old),
            );
            write_config(&self.home, &migrated).await?;
        }

        let parsed = parse_command_file(&content, &new_text)
            .ok_or_else(|| "Failed to parse written command file".to_owned())?;
        let name = command_name(&root, &new_path, descriptor);
        // 回包的 `enabled` 是**重新读一次**覆盖表（TS 同），而不是沿用上面的快照。
        let enabled = is_enabled(
            &enabled_overrides(&read_config(&self.home).await),
            &new_text,
        );
        Ok(json!({
            "command": user_command(
                &parsed,
                &name,
                descriptor,
                &root,
                scope,
                project_path.as_deref(),
                enabled,
            ),
        }))
    }

    pub async fn delete_command_file(&self, params: &Value) -> Result<(), String> {
        let file_path = params["filePath"]
            .as_str()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "Missing filePath".to_owned())?;
        match tokio::fs::remove_file(file_path).await {
            // 文件不存在当作成功（TS 同）。
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("{error}")),
            Ok(()) => {}
        }
        let config = read_config(&self.home).await;
        write_config(&self.home, &set_override(&config, file_path, true)).await
    }

    pub async fn set_command_enabled(&self, params: &Value) -> Result<(), String> {
        let file_path = params["filePath"]
            .as_str()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "Missing filePath".to_owned())?;
        let enabled = params["enabled"]
            .as_bool()
            .ok_or_else(|| "Missing enabled".to_owned())?;
        let config = read_config(&self.home).await;
        write_config(&self.home, &set_override(&config, file_path, enabled)).await
    }
}

/// TS `CommandConfig`（写面只用到这几个字段）。
struct Config {
    name: String,
    prompt: String,
    description: Option<String>,
    argument_hint: Option<String>,
}

fn file_config(raw: &Value) -> Result<Config, String> {
    let name = raw["name"]
        .as_str()
        .ok_or_else(|| "config.name is required".to_owned())?
        .to_owned();
    let prompt = raw["prompt"]
        .as_str()
        .ok_or_else(|| "config.prompt is required".to_owned())?
        .to_owned();
    Ok(Config {
        name,
        prompt,
        description: raw["description"].as_str().map(str::to_owned),
        argument_hint: raw["argumentHint"].as_str().map(str::to_owned),
    })
}

/// TS `getWritableCommandConfig`：不支持 `argument-hint` 的来源丢掉该字段。
fn writable<'a>(config: &'a Config, descriptor: &SourceDescriptor) -> FileConfig<'a> {
    FileConfig {
        prompt: &config.prompt,
        description: config.description.as_deref(),
        argument_hint: descriptor
            .supports_argument_hint
            .then_some(config.argument_hint.as_deref())
            .flatten(),
    }
}

/// TS `getCommandFileName`：去掉一个前导 `/`，再拼扩展名。
fn command_file_name(config: &Config, descriptor: &SourceDescriptor) -> Result<String, String> {
    let raw = config.name.strip_prefix('/').unwrap_or(&config.name);
    if raw.is_empty() {
        return Err("Command name is required".to_owned());
    }
    let relative = if descriptor.namespace_separator == ":" {
        raw.replace(':', "/")
    } else {
        raw.to_owned()
    };
    Ok(format!("{relative}{}", descriptor.file_extension))
}

/// TS `getCommandName`：相对 root 的路径、去扩展名、按分隔符拼命名空间。
pub(super) fn command_name(root: &Path, file_path: &Path, descriptor: &SourceDescriptor) -> String {
    let relative = file_path
        .strip_prefix(root)
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|_| file_path.to_string_lossy().into_owned());
    let stem = if relative.len() >= descriptor.file_extension.len()
        && relative[relative.len() - descriptor.file_extension.len()..]
            .eq_ignore_ascii_case(descriptor.file_extension)
    {
        &relative[..relative.len() - descriptor.file_extension.len()]
    } else {
        &relative
    };
    let joined = stem
        .split(['\\', '/'])
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(descriptor.namespace_separator);
    format!("/{joined}")
}

/// TS `buildCommandLocation`（`source` 取描述表的 `directorySource`，`scope` 是 user/project 二值）。
pub(super) fn location(
    descriptor: &SourceDescriptor,
    root: &Path,
    scope: &str,
    project_path: Option<&str>,
) -> Value {
    let mut location = Map::new();
    location.insert(
        "source".to_owned(),
        Value::from(descriptor.directory_source),
    );
    location.insert(
        "scope".to_owned(),
        Value::from(if scope == "project" {
            "project"
        } else {
            "user"
        }),
    );
    location.insert("directoryPath".to_owned(), Value::from(path_string(root)));
    if let Some(project) = project_path {
        location.insert("projectPath".to_owned(), Value::from(project));
    }
    Value::Object(location)
}

/// `{...parsed, name, agentSource, location, id, source, enabled, scope, projectPath?}`。
fn user_command(
    parsed: &super::parser::ParsedCommandFile,
    name: &str,
    descriptor: &SourceDescriptor,
    root: &Path,
    scope: &str,
    project_path: Option<&str>,
    enabled: bool,
) -> Value {
    let mut command = parsed_fields(parsed);
    command.insert("name".to_owned(), Value::from(name));
    command.insert(
        "agentSource".to_owned(),
        Value::from(descriptor.agent_source),
    );
    let location = location(descriptor, root, scope, project_path);
    command.insert("location".to_owned(), location.clone());
    command.insert(
        "id".to_owned(),
        Value::from(format!(
            "{}:{}:{}:{}",
            descriptor.agent_source,
            location["source"].as_str().unwrap_or_default(),
            scope,
            name
        )),
    );
    command.insert("source".to_owned(), Value::from("user"));
    command.insert("enabled".to_owned(), Value::Bool(enabled));
    command.insert("scope".to_owned(), Value::from(scope));
    if let Some(project) = project_path {
        command.insert("projectPath".to_owned(), Value::from(project));
    }
    Value::Object(command)
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

async fn write_file(path: &Path, content: &str) -> Result<(), String> {
    tokio::fs::write(path, content)
        .await
        .map_err(|error| format!("{error}"))
}

#[cfg(test)]
#[path = "write_tests.rs"]
mod tests;
