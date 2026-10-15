//! `output-style` channel（docs/specs/rust-u2-user-config-services.md §1）。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use escode_host_services::git::paths::{is_js_space, js_trim};
use serde_json::{Map, Value, json};

use crate::channel::Channel;
use crate::collation::compare_names;
use crate::session::{MethodHandler, RpcError, handler_rpc};
use crate::wire::Wire;

const BUILT_IN: [(&str, &str, &str); 3] = [
    (
        "default",
        "Default",
        "Completes coding tasks efficiently and provides concise responses",
    ),
    (
        "explanatory",
        "Explanatory",
        "Explains implementation choices and codebase patterns",
    ),
    (
        "learning",
        "Learning",
        "Pauses and asks you to write small pieces of code for hands-on practice",
    ),
];

#[derive(Clone)]
struct Dirs {
    styles: PathBuf,
    claude: PathBuf,
}

impl Dirs {
    fn new(home: &Path) -> Self {
        let claude = home.join(".claude");
        Self {
            styles: claude.join("output-styles"),
            claude,
        }
    }

    fn settings(&self) -> PathBuf {
        self.claude.join("settings.json")
    }
}

fn io_error(error: &std::io::Error) -> RpcError {
    RpcError::new("Error", error.to_string())
}

/// TS `readJsonFile`：不存在或损坏都按“没有”处理。
async fn read_settings(path: &Path) -> Option<Map<String, Value>> {
    let text = tokio::fs::read_to_string(path).await.ok()?;
    match serde_json::from_str::<Value>(&text).ok()? {
        Value::Object(map) => Some(map),
        // JSON 合法但不是对象（如数组）：TS 照样把它当 settings 展开；对象展开数组得到索引键，等价于“没有字段”。
        _ => Some(Map::new()),
    }
}

async fn active_style(dirs: &Dirs) -> Option<String> {
    read_settings(&dirs.settings())
        .await?
        .get("outputStyle")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

/// TS `parseMarkdownStyle`：首个 `name:` / `description:` 行（`^…$` 多行匹配）。
fn parse_style(content: &str, filename: &str) -> (String, String) {
    let field = |key: &str| -> Option<String> {
        let prefix = format!("{key}:");
        // 与 JS `^key:\s*(.+)$`（m）等价：某行以 key: 开头，`\s*` 可跨行吃空白，`.+` 取到行尾。
        let mut offset = 0;
        for line in content.split('\n') {
            if let Some(rest) = line.strip_prefix(&prefix) {
                let after = &content[offset + prefix.len()..];
                let trimmed = after.trim_start_matches(is_js_space);
                let value = trimmed.split(['\n', '\r']).next().unwrap_or_default();
                if !value.is_empty() {
                    return Some(js_trim(value).to_owned());
                }
                let _ = rest;
            }
            offset += line.len() + 1;
        }
        None
    };
    let mut name = field("name").unwrap_or_default();
    if name.is_empty() {
        name = filename.strip_suffix(".md").unwrap_or(filename).to_owned();
    }
    if name.is_empty() {
        name = "Custom Style".to_owned();
    }
    (name, field("description").unwrap_or_default())
}

async fn list_styles(dirs: &Dirs) -> Result<Wire, RpcError> {
    let active = active_style(dirs)
        .await
        .unwrap_or_else(|| "default".to_owned());
    let mut custom: Vec<(String, Value)> = Vec::new();
    if let Ok(mut entries) = tokio::fs::read_dir(&dirs.styles).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let file_name = entry.file_name().to_string_lossy().into_owned();
            let is_file = entry.file_type().await.is_ok_and(|kind| kind.is_file());
            if !is_file || !file_name.ends_with(".md") {
                continue;
            }
            let path = dirs.styles.join(&file_name);
            let Ok(content) = tokio::fs::read_to_string(&path).await else {
                continue;
            };
            let (name, description) = parse_style(&content, &file_name);
            // JS `replace(".md", "")` 只换第一处。
            let id = format!("custom-{}", file_name.replacen(".md", "", 1));
            let enabled = id == active;
            custom.push((
                name.clone(),
                json!({
                    "id": id, "name": name, "description": description, "content": content,
                    "isBuiltIn": false, "enabled": enabled, "filePath": path.to_string_lossy(),
                }),
            ));
        }
    }
    custom.sort_by(|left, right| compare_names(&left.0, &right.0));
    let mut styles: Vec<Value> = BUILT_IN
        .iter()
        .map(|(id, name, description)| {
            json!({
                "id": id, "name": name, "description": description, "content": "",
                "isBuiltIn": true, "enabled": *id == active,
            })
        })
        .collect();
    styles.extend(custom.into_iter().map(|(_, style)| style));
    Ok(Wire::Object(json!({"styles": styles})))
}

fn style_file(dirs: &Dirs, id: &str) -> PathBuf {
    let base = id.strip_prefix("custom-").unwrap_or(id);
    dirs.styles.join(format!("{base}.md"))
}

fn style_body(config: &Value) -> String {
    format!(
        "name: {}\ndescription: {}\n\n{}",
        config["name"].as_str().unwrap_or_default(),
        config["description"].as_str().unwrap_or_default(),
        config["content"].as_str().unwrap_or_default()
    )
}

fn params_of(arg: Wire) -> Value {
    arg.into_args()
        .into_iter()
        .next()
        .and_then(Wire::into_object)
        .unwrap_or(Value::Null)
}

fn method<F, Fut>(dirs: &Dirs, run: F) -> MethodHandler
where
    F: Fn(Dirs, Value) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<Wire, RpcError>> + Send + 'static,
{
    let dirs = dirs.clone();
    let run = Arc::new(run);
    handler_rpc(move |arg: Wire| {
        let (dirs, run) = (dirs.clone(), Arc::clone(&run));
        Box::pin(async move { run(dirs, params_of(arg)).await })
    })
}

pub(crate) fn output_style_channel(home: &Path) -> Channel {
    let dirs = Dirs::new(home);
    Channel::new(
        "output-style",
        vec![
            (
                "listStyles",
                method(&dirs, |dirs, _| async move { list_styles(&dirs).await }),
            ),
            (
                "addStyle",
                method(&dirs, |dirs, params| async move {
                    tokio::fs::create_dir_all(&dirs.styles)
                        .await
                        .map_err(|e| io_error(&e))?;
                    let name = params["config"]["name"]
                        .as_str()
                        .unwrap_or_default()
                        .to_lowercase();
                    let mut slug = String::new();
                    let mut in_space = false;
                    for ch in name.chars() {
                        if is_js_space(ch) {
                            if !in_space {
                                slug.push('-');
                            }
                            in_space = true;
                        } else {
                            slug.push(ch);
                            in_space = false;
                        }
                    }
                    let path = dirs.styles.join(format!("{slug}.md"));
                    tokio::fs::write(&path, style_body(&params["config"]))
                        .await
                        .map_err(|e| io_error(&e))?;
                    Ok(Wire::Undefined)
                }),
            ),
            (
                "updateStyle",
                method(&dirs, |dirs, params| async move {
                    let path = style_file(&dirs, params["id"].as_str().unwrap_or_default());
                    tokio::fs::write(&path, style_body(&params["config"]))
                        .await
                        .map_err(|e| io_error(&e))?;
                    Ok(Wire::Undefined)
                }),
            ),
            (
                "deleteStyle",
                method(&dirs, |dirs, params| async move {
                    let path = style_file(&dirs, params["id"].as_str().unwrap_or_default());
                    match tokio::fs::remove_file(&path).await {
                        Ok(()) => Ok(Wire::Undefined),
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                            Ok(Wire::Undefined)
                        }
                        Err(error) => Err(io_error(&error)),
                    }
                }),
            ),
            (
                "getUserStylesDirectory",
                method(&dirs, |dirs, _| async move {
                    let _ = tokio::fs::create_dir_all(&dirs.styles).await;
                    Ok(Wire::Object(json!({"path": dirs.styles.to_string_lossy()})))
                }),
            ),
            (
                "setActiveStyle",
                method(&dirs, |dirs, params| async move {
                    tokio::fs::create_dir_all(&dirs.claude)
                        .await
                        .map_err(|e| io_error(&e))?;
                    let mut settings = read_settings(&dirs.settings()).await.unwrap_or_default();
                    settings.insert("outputStyle".to_owned(), params["styleId"].clone());
                    let text = serde_json::to_string_pretty(&Value::Object(settings))
                        .map_err(|e| RpcError::new("Error", e.to_string()))?;
                    tokio::fs::write(dirs.settings(), text)
                        .await
                        .map_err(|e| io_error(&e))?;
                    Ok(Wire::Undefined)
                }),
            ),
            (
                "getActiveStyle",
                method(&dirs, |dirs, _| async move {
                    let style = active_style(&dirs).await;
                    Ok(Wire::Object(json!({"styleId": style})))
                }),
            ),
        ],
    )
}

#[cfg(test)]
#[path = "domains_output_style_tests.rs"]
mod tests;
