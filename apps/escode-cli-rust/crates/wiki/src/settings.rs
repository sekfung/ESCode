//! Wiki 配置与根目录（spec §3、§12）。读取与 ESCode 其他能力相同的配置文件：
//! 用户层 `~/.escode/cli/config.json`，项目层（Git 根 → cwd 的 `escode.json`、`.escode/config.json`）。
//! 安全：项目层来自仓库内容（不可信），只允许调整检索/格式/开关，**不能**设置 `root`（重定向 Global Wiki 存储）
//! 或 `converters`/`ocr`（会执行外部命令）；这些键只认用户层。
//! Global Wiki 根目录：用户层 `wiki.root`，否则 `<storage>/wiki`（storage = ESCODE_STORAGE_DIR → 用户层 storage.dir → ~/.escode）。
//! Project Wiki 不在这里：它固定在项目工作区的 `.escode/wiki/`（store::PROJECT_WIKI_DIR）。
use escode_cli_domain::wiki::config::WikiConfig;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

const MAX_CONFIG_BYTES: u64 = 4 * 1024 * 1024;
const PROJECT_FORBIDDEN_KEYS: [&str; 3] = ["root", "converters", "ocr"];

#[derive(Clone, Debug)]
pub struct Settings {
    pub config: WikiConfig,
    /// Global Wiki 根目录（即 Global 存储本身）。
    pub root: PathBuf,
    /// 被忽略的项目层键（诊断用）。
    pub ignored_project_keys: Vec<String>,
}

pub fn home() -> PathBuf {
    std::env::var_os("HOME")
        .filter(|v| !v.is_empty())
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_default()
}

async fn json_file(path: &Path) -> Value {
    let Ok(meta) = tokio::fs::metadata(path).await else {
        return json!({});
    };
    if !meta.is_file() || meta.len() > MAX_CONFIG_BYTES {
        return json!({});
    }
    tokio::fs::read(path)
        .await
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_else(|| json!({}))
}

fn merge(base: &mut Value, next: &Value) {
    match (base.as_object_mut(), next.as_object()) {
        (Some(base), Some(next)) => {
            for (key, value) in next {
                merge(base.entry(key.clone()).or_insert(Value::Null), value);
            }
        }
        _ => *base = next.clone(),
    }
}

/// `~/` 与相对路径都相对于（可注入的）home 目录展开。
fn expand(home_dir: &Path, path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => home_dir.join(rest),
        None if Path::new(path).is_absolute() => PathBuf::from(path),
        None => home_dir.join(path),
    }
}

pub async fn load(cwd: &Path, home_dir: &Path) -> Settings {
    let user = json_file(&home_dir.join(".escode").join("cli").join("config.json")).await;
    let mut wiki = user.get("wiki").cloned().unwrap_or_else(|| json!({}));
    let mut ignored = vec![];
    let mut dirs = vec![];
    for dir in cwd.ancestors() {
        dirs.push(dir.to_owned());
        if tokio::fs::metadata(dir.join(".git")).await.is_ok() {
            break;
        }
    }
    for dir in dirs.iter().rev() {
        for file in ["escode.json", ".escode/config.json"] {
            let layer = json_file(&dir.join(file)).await;
            let Some(mut project) = layer.get("wiki").cloned() else {
                continue;
            };
            if let Some(object) = project.as_object_mut() {
                for key in PROJECT_FORBIDDEN_KEYS {
                    if object.remove(key).is_some() {
                        ignored.push(format!("{}: wiki.{key}", dir.join(file).display()));
                    }
                }
            }
            merge(&mut wiki, &project);
        }
    }
    let config = WikiConfig::from_value(&wiki);
    let storage = std::env::var("ESCODE_STORAGE_DIR")
        .ok()
        .or_else(|| user["storage"]["dir"].as_str().map(str::to_owned))
        .map(|p| expand(home_dir, &p))
        .unwrap_or_else(|| home_dir.join(".escode"));
    let root = config
        .root
        .as_deref()
        .map(|r| expand(home_dir, r))
        .unwrap_or_else(|| storage.join("wiki"));
    Settings {
        config,
        root,
        ignored_project_keys: ignored,
    }
}
