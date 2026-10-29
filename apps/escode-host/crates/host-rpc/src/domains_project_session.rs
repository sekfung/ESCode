//! `project.prepareSession` / `project.readDirectory` 与路径、主目录状态辅助
//! （上游 `project/index.ts` 的本地 Host 子集）。

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use tokio::sync::Mutex;

use crate::domains_task_support::ReadContext;
use crate::session::ChannelResult;
use crate::wire::Wire;

fn rejected(code: &str) -> (String, String) {
    ("Rejected".to_owned(), code.to_owned())
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        })
}

/// `node:path.normalize` 的词法归一（折叠 `.`/重复分隔符，按词法消解 `..`，保留根前缀）。
pub(crate) fn normalize_path(path: &str) -> String {
    let mut segments: Vec<String> = Vec::new();
    let mut prefix = String::new();
    let mut rooted = false;
    for component in Path::new(path).components() {
        match component {
            Component::Prefix(value) => prefix = value.as_os_str().to_string_lossy().into_owned(),
            Component::RootDir => rooted = true,
            Component::CurDir => {}
            Component::ParentDir => {
                // 绝对路径的 `..` 到根为止；相对路径保留无法消解的前缀。
                if segments.last().is_some_and(|last| last != "..") {
                    segments.pop();
                } else if !rooted {
                    segments.push("..".to_owned());
                }
            }
            Component::Normal(value) => segments.push(value.to_string_lossy().into_owned()),
        }
    }
    let joined = segments.join(std::path::MAIN_SEPARATOR_STR);
    if rooted {
        format!("{prefix}{}{joined}", std::path::MAIN_SEPARATOR)
    } else if joined.is_empty() {
        ".".to_owned()
    } else {
        joined
    }
}

pub(crate) fn is_absolute(path: &str) -> bool {
    Path::new(path).is_absolute()
}

/// 上游 `readPrimaryRootStatus`：目录存在且是目录 = available；ENOENT/ENOTDIR = missing；其余 = unavailable。
async fn primary_root_status(path: &str) -> &'static str {
    match tokio::fs::metadata(path).await {
        Ok(metadata) if metadata.is_dir() => "available",
        Ok(_) => "missing",
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => "missing",
        Err(_) => "unavailable",
    }
}

/// 上游 `projectPrimaryRootStatuses`：每次读取重新派生，不把文件系统事实固化进 SQLite。
pub(crate) async fn decorate_projects(projects: &mut [Value]) {
    let mut checks: HashMap<String, &'static str> = HashMap::new();
    for project in projects {
        if project["deleted"] == true {
            continue;
        }
        let primary = project["roots"]
            .as_array()
            .and_then(|roots| roots.iter().find(|root| root["isPrimary"] == true))
            .and_then(|root| root["rootPath"].as_str().map(str::to_owned));
        let Some(primary) = primary else { continue };
        let status = match checks.get(&primary) {
            Some(status) => *status,
            None => {
                let status = primary_root_status(&primary).await;
                checks.insert(primary, status);
                status
            }
        };
        project["primaryRootStatus"] = json!(status);
    }
}

/// 上游 `readDirectory`：缺省 home；绝对路径 + normalize 后委托文件服务。
pub(crate) async fn read_directory(home: &Path, params: &Value) -> ChannelResult {
    if let Some(host_id) = params["hostId"].as_str().filter(|value| !value.is_empty())
        && host_id != "local"
    {
        return Err(rejected("project.host-offline"));
    }
    let requested = match params.get("path") {
        None | Some(Value::Null) => home.to_string_lossy().into_owned(),
        Some(Value::String(path)) if !path.is_empty() => path.clone(),
        Some(_) => return Err(rejected("project.invalid-request")),
    };
    if !is_absolute(&requested) {
        return Err(rejected("project.absolute-path-required"));
    }
    let path = normalize_path(&requested);
    let mut readdir_params = json!({"path": path});
    if let Some(include_hidden) = params["includeHidden"].as_bool() {
        readdir_params["includeHidden"] = json!(include_hidden);
    }
    let entries = crate::domains_file_readdir::readdir(&readdir_params)?;
    let entries = match entries {
        Wire::Array(values) => Value::Array(values.into_iter().map(wire_value).collect()),
        other => wire_value(other),
    };
    Ok(Wire::Object(json!({"path": path, "entries": entries})))
}

fn wire_value(wire: Wire) -> Value {
    match wire {
        Wire::Undefined => Value::Null,
        Wire::Str(value) => json!(value),
        Wire::Int(value) => json!(value),
        Wire::Object(value) => value,
        Wire::Array(values) => Value::Array(values.into_iter().map(wire_value).collect()),
        Wire::Bytes { data, .. } => json!(data),
    }
}

fn roots_of(project: &Value) -> Vec<String> {
    project["roots"]
        .as_array()
        .map(|roots| {
            roots
                .iter()
                .filter_map(|root| root["rootPath"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// 上游 `prepareSession`：固化创建 cwd，空项目才建默认目录并复检主目录。
///
/// 同一进程内串行化（上游用 per-project flight；串行得到同样的可观察结果，
/// 且不会让并发调用各建一个 `draft-<id>` 目录）。
pub(crate) async fn prepare_session(
    read: &ReadContext,
    workspace_dir: &Path,
    params: &Value,
    lock: &Mutex<()>,
) -> ChannelResult {
    let draft_id = match params.get("draftId") {
        None | Some(Value::Null) => None,
        Some(Value::String(value)) => match uuid::Uuid::parse_str(value) {
            Ok(_) => Some(value.clone()),
            Err(_) => return Err(rejected("project.invalid-draft-target")),
        },
        Some(_) => return Err(rejected("project.invalid-draft-target")),
    };
    let project_id = params["projectId"]
        .as_str()
        .filter(|value| !value.is_empty());
    let reuse_draft = params["reuseDraft"].as_bool().unwrap_or(false);
    if (draft_id.is_some() && project_id.is_some()) || (reuse_draft && draft_id.is_none()) {
        return Err(rejected("project.invalid-draft-target"));
    }
    if let Some(host_id) = params["hostId"].as_str().filter(|value| !value.is_empty())
        && host_id != "local"
    {
        return Err(rejected("project.host-offline"));
    }

    let _guard = lock.lock().await;
    let Some(project_id) = project_id else {
        // 草稿 ID 由客户端先落盘：重复点击、回包丢失与重启重试都落到同一目录。
        let name = match &draft_id {
            Some(draft) => format!("draft-{draft}"),
            None => format!("{}-{}", now_ms(), uuid::Uuid::new_v4()),
        };
        let path = workspace_dir.join(name);
        if reuse_draft {
            // 已交付目录被移走时只报告缺失，不能用 mkdir 静默重建空目录。
            if !tokio::fs::metadata(&path)
                .await
                .is_ok_and(|metadata| metadata.is_dir())
            {
                return Err(rejected("project.directory-required"));
            }
        } else if let Err(error) = tokio::fs::create_dir_all(&path).await {
            return Err((
                "NodeIoError".to_owned(),
                format!("mkdir {}: {error}", path.display()),
            ));
        }
        return Ok(Wire::Object(json!({
            "workspacePath": path.to_string_lossy(),
            "additionalDirectories": [],
            "projectId": Value::Null,
        })));
    };
    let project_id = project_id.to_owned();

    let tree = read
        .run({
            let project_id = project_id.clone();
            move |index| {
                let tree = index
                    .project_read_tree(false, false, false)
                    .map_err(|error| error.to_string())?;
                Ok(tree
                    .get("projects")
                    .and_then(Value::as_array)
                    .and_then(|projects| {
                        projects
                            .iter()
                            .find(|item| item["id"] == project_id)
                            .cloned()
                    }))
            }
        })
        .await?;
    let Some(mut project) = tree else {
        return Err(rejected("project.not-found"));
    };
    if project["roots"].as_array().is_none_or(Vec::is_empty) {
        let path = workspace_dir.join(&project_id);
        if let Err(error) = tokio::fs::create_dir_all(&path).await {
            return Err((
                "NodeIoError".to_owned(),
                format!("mkdir {}: {error}", path.display()),
            ));
        }
        // 空判断与 INSERT 共用写事务，避免覆盖另一窗口刚编辑的 roots。
        project = read
            .run({
                let project_id = project_id.clone();
                let path = path.to_string_lossy().into_owned();
                move |index| {
                    index
                        .project_initialize_root(&json!({"id": project_id, "path": path}))
                        .map_err(|error| error.to_string())
                }
            })
            .await?;
    }
    let workspace_path = project["roots"]
        .as_array()
        .and_then(|roots| roots.iter().find(|root| root["isPrimary"] == true))
        .and_then(|root| root["rootPath"].as_str())
        .map(str::to_owned);
    let Some(workspace_path) = workspace_path else {
        return Err(rejected("project.primary-root-missing"));
    };
    // 项目树与点击之间主目录仍可能被移走；创建入口必须复检，不能改选附加目录。
    match primary_root_status(&workspace_path).await {
        "available" => {}
        "missing" => return Err(rejected("project.primaryRootMissing")),
        _ => return Err(rejected("project.primaryRootUnavailable")),
    }
    // 目录检查会让出执行权；项目在此期间删除时不得返回可用于首发的旧归属。
    let alive = read
        .run({
            let project_id = project_id.clone();
            move |index| {
                index
                    .project_read_tree(false, false, false)
                    .map(|tree| {
                        tree["projects"].as_array().is_some_and(|projects| {
                            projects
                                .iter()
                                .any(|item| item["id"] == project_id && item["deleted"] != true)
                        })
                    })
                    .map_err(|error| error.to_string())
            }
        })
        .await?;
    if !alive {
        return Err(rejected("project.not-found"));
    }
    let additional = roots_of(&project)
        .into_iter()
        .filter(|root| root != &workspace_path)
        .collect::<Vec<_>>();
    Ok(Wire::Object(json!({
        "workspacePath": workspace_path,
        "additionalDirectories": additional,
        "projectId": project_id,
    })))
}

/// `saveProject` 的目录校验与归一化；返回归一化后的 roots / primaryRoot。
pub(crate) async fn validate_save_project(input: &Value) -> Result<Value, (String, String)> {
    if let Some(host_id) = input["hostId"].as_str().filter(|value| !value.is_empty())
        && host_id != "local"
    {
        return Err(rejected("project.host-offline"));
    }
    if input["id"].as_str().is_some_and(|value| !value.is_empty())
        && input["restoreProjectId"]
            .as_str()
            .is_some_and(|value| !value.is_empty())
    {
        return Err(rejected("project.ambiguous-save"));
    }
    if input["restoreProjectId"]
        .as_str()
        .is_some_and(|value| !value.is_empty())
        && input.get("roots").is_none_or(Value::is_null)
    {
        return Err(rejected("project.restore-roots-required"));
    }
    let mut normalized = input.clone();
    if let Some(roots) = input.get("roots").filter(|value| !value.is_null()) {
        let Some(roots) = roots.as_array() else {
            return Err(rejected("project.absolute-path-required"));
        };
        let mut unique: Vec<String> = Vec::new();
        for root in roots {
            let Some(root) = root.as_str().filter(|value| !value.is_empty()) else {
                return Err(rejected("project.absolute-path-required"));
            };
            if !is_absolute(root) {
                return Err(rejected("project.absolute-path-required"));
            }
            let root = normalize_path(root);
            if !tokio::fs::metadata(&root)
                .await
                .is_ok_and(|metadata| metadata.is_dir())
            {
                return Err(rejected("project.directory-required"));
            }
            if !unique.contains(&root) {
                unique.push(root);
            }
        }
        normalized["roots"] = json!(unique);
    }
    if let Some(primary) = input["primaryRoot"]
        .as_str()
        .filter(|value| !value.is_empty())
    {
        if !is_absolute(primary) {
            return Err(rejected("project.absolute-path-required"));
        }
        normalized["primaryRoot"] = json!(normalize_path(primary));
    }
    Ok(normalized)
}

/// `moveTask` 的目录校验：确认增加缺失 root 时也校验目录，不能绕过存在性约束。
pub(crate) async fn validate_move_task(input: &Value) -> Result<(), (String, String)> {
    if let Some(host_id) = input["hostId"].as_str().filter(|value| !value.is_empty())
        && host_id != "local"
    {
        return Err(rejected("project.host-offline"));
    }
    if input["addMissingRoot"].as_bool() == Some(true) {
        let path = input["task"]["workspacePath"].as_str().unwrap_or_default();
        if !tokio::fs::metadata(path)
            .await
            .is_ok_and(|metadata| metadata.is_dir())
        {
            return Err(rejected("project.directory-required"));
        }
    }
    Ok(())
}

/// `workspaceDirectory` = `<dataBaseDir>/.escode/workspace/default`（与 conversation workspace 同源）。
pub(crate) fn workspace_directory(data_base_dir: &Path) -> PathBuf {
    data_base_dir
        .join(".escode")
        .join("workspace")
        .join("default")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_matches_node_path_normalize() {
        // Node `path.normalize` 也是平台相关的：Windows 输出反斜杠，POSIX 输出斜杠。
        let sep = std::path::MAIN_SEPARATOR;
        assert_eq!(normalize_path("/a//b/./c"), format!("{sep}a{sep}b{sep}c"));
        assert_eq!(normalize_path("/a/../b"), format!("{sep}b"));
        assert_eq!(normalize_path("a/../../b"), format!("..{sep}b"));
        assert_eq!(normalize_path("/.."), sep.to_string());
        assert_eq!(normalize_path(""), ".");
    }
}
