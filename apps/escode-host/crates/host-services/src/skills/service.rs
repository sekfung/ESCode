//! `skills` 服务（TS `createSkillsService`）：list / setEnabled / buildPromptContext / copyToCommon / removeFromCommon / deleteSkill。

use std::path::{Path, PathBuf};

use escode_cli_domain::json_order::Json;
use serde_json::{Value, json};
use tokio::sync::Mutex;

use super::discover::{self, Compare, Discovered};
use super::policy::SKILL_FILE_NAME;
use super::service_support::{activated_block, copy_tree, mentioned_names, remove_all};

#[derive(Debug)]
pub struct SkillsError(pub String);

impl std::fmt::Display for SkillsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SkillsError {}

type Result<T> = std::result::Result<T, SkillsError>;

fn fail<T>(message: impl Into<String>) -> Result<T> {
    Err(SkillsError(message.into()))
}

pub struct SkillsService {
    home: PathBuf,
    compare: Compare,
    /// TS `Boolean(process.env.ESCODE_PROCESS_LABEL)`：桌面运行时才有用户级技能。
    desktop: bool,
    /// 串行化 setEnabled 的读-改-写。
    write_queue: Mutex<()>,
}

fn normalize_config_path(path: &str) -> String {
    path.replace('\\', "/")
}

impl SkillsService {
    pub fn new(home: PathBuf, compare: Compare, desktop: bool) -> Self {
        Self {
            home,
            compare,
            desktop,
            write_queue: Mutex::new(()),
        }
    }

    fn cli_config_path(&self) -> PathBuf {
        self.home.join(".escode").join("cli").join("config.json")
    }

    async fn config(&self) -> Value {
        match tokio::fs::read_to_string(self.cli_config_path()).await {
            Ok(text) => match serde_json::from_str::<Value>(&text) {
                Ok(value) if value.is_object() => value,
                _ => json!({}),
            },
            Err(_) => json!({}),
        }
    }

    fn capability(&self) -> Value {
        if self.desktop {
            json!({"userScopeAvailable": true})
        } else {
            json!({"userScopeAvailable": false, "userScopeReason": "desktop_only"})
        }
    }

    async fn discover(&self, workspace: &str) -> Discovered {
        let config = self.config().await;
        let (home, compare, include_user, workspace) = (
            self.home.clone(),
            self.compare.clone(),
            self.desktop,
            PathBuf::from(workspace),
        );
        tokio::task::spawn_blocking(move || {
            discover::discover(&workspace, include_user, &home, &config, &compare)
        })
        .await
        .unwrap_or(Discovered {
            skills: Vec::new(),
            diagnostics: Vec::new(),
        })
    }

    fn enabled_map(config: &Value) -> std::collections::BTreeMap<String, bool> {
        config
            .get("skills")
            .and_then(Value::as_object)
            .map(|skills| {
                skills
                    .iter()
                    .filter_map(|(path, value)| {
                        value
                            .get("enable")
                            .and_then(Value::as_bool)
                            .map(|e| (normalize_config_path(path), e))
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub async fn list(&self, workspace: &str) -> Value {
        let Discovered {
            mut skills,
            diagnostics,
        } = self.discover(workspace).await;
        let enabled = Self::enabled_map(&self.config().await);
        for skill in &mut skills {
            let path = normalize_config_path(skill["path"].as_str().unwrap_or_default());
            skill["enabled"] = Value::Bool(enabled.get(&path).copied().unwrap_or(true));
        }
        json!({"skills": skills, "capability": self.capability(), "diagnostics": diagnostics})
    }

    async fn find(&self, workspace: &str, skill_id: &str, listed: bool) -> Result<Value> {
        let skills = if listed {
            self.list(workspace).await["skills"].clone()
        } else {
            Value::Array(self.discover(workspace).await.skills)
        };
        skills
            .as_array()
            .and_then(|list| list.iter().find(|s| s["id"] == skill_id).cloned())
            .map_or_else(|| fail(format!("Skill not found: {skill_id}")), Ok)
    }

    pub async fn set_enabled(&self, workspace: &str, skill_id: &str, enabled: bool) -> Result<()> {
        let _queue = self.write_queue.lock().await;
        let skill = self.find(workspace, skill_id, false).await?;
        let config_value = self.config().await;
        let mut map = Self::enabled_map(&config_value);
        map.insert(
            normalize_config_path(skill["path"].as_str().unwrap_or_default()),
            enabled,
        );
        self.write_enabled_map(map).await
    }

    /// TS `writeSkillEnabledMap`：开启态是默认值，删除 override；关闭态写 `{enable:false}`；保持配置文件其它键序。
    async fn write_enabled_map(
        &self,
        next: std::collections::BTreeMap<String, bool>,
    ) -> Result<()> {
        let text = tokio::fs::read_to_string(self.cli_config_path()).await.ok();
        let mut config = text
            .as_deref()
            .and_then(Json::parse)
            .filter(Json::is_object)
            .unwrap_or_else(Json::object);
        let mut skills = match config.get("skills") {
            Some(existing) if existing.is_object() => existing.clone(),
            _ => Json::object(),
        };
        let mut entries: Vec<(String, bool)> = next.into_iter().collect();
        entries.sort_by(|(l, _), (r, _)| (self.compare)(l, r));
        for (path, enable) in entries {
            let key = normalize_config_path(&path);
            if enable {
                skills.remove(&key);
            } else {
                let mut value = Json::object();
                value.set("enable", Json::Bool(false));
                skills.set(&key, value);
            }
        }
        if matches!(&skills, Json::Object(entries) if !entries.is_empty()) {
            config.set("skills", skills);
        } else {
            config.remove("skills");
        }
        let path = self.cli_config_path();
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| SkillsError(e.to_string()))?;
        }
        tokio::fs::write(&path, format!("{}\n", config.pretty()))
            .await
            .map_err(|e| SkillsError(e.to_string()))
    }

    pub async fn build_prompt_context(
        &self,
        workspace: &str,
        identity: Option<&str>,
        prompt: &str,
    ) -> Result<Value> {
        let names = mentioned_names(prompt);
        if names.is_empty() {
            return Ok(json!({"prompt": prompt, "activatedSkillNames": []}));
        }
        let listed = self.list(workspace).await;
        let activated: Vec<&Value> = listed["skills"]
            .as_array()
            .map(|skills| {
                skills
                    .iter()
                    .filter(|s| {
                        s["enabled"] == true
                            && names.contains(s["name"].as_str().unwrap_or_default())
                    })
                    .collect()
            })
            .unwrap_or_default();
        if activated.is_empty() {
            return Ok(json!({"prompt": prompt, "activatedSkillNames": []}));
        }
        let activated_names: Vec<String> = activated
            .iter()
            .map(|s| s["name"].as_str().unwrap_or_default().to_owned())
            .collect();
        self.append_audit(workspace, identity, &activated_names)
            .await?;
        Ok(json!({
            "prompt": format!("{prompt}\n\n{}", activated_block(&activated)),
            "activatedSkillNames": activated_names,
        }))
    }

    async fn append_audit(
        &self,
        workspace: &str,
        identity: Option<&str>,
        names: &[String],
    ) -> Result<()> {
        use tokio::io::AsyncWriteExt;
        let dir = self.home.join(".escode").join("v2");
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|e| SkillsError(e.to_string()))?;
        let created = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64);
        let line = format!(
            "{}\n",
            json!({"createdAt": created, "workspacePath": workspace, "workspaceIdentity": identity, "activatedSkillNames": names})
        );
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("skills-audit.log"))
            .await
            .map_err(|e| SkillsError(e.to_string()))?;
        file.write_all(line.as_bytes())
            .await
            .map_err(|e| SkillsError(e.to_string()))
    }

    pub async fn copy_to_common(&self, workspace: &str, skill_id: &str) -> Result<Value> {
        let skill = self.find(workspace, skill_id, true).await?;
        let path = PathBuf::from(skill["path"].as_str().unwrap_or_default());
        let source = path.parent().map(Path::to_path_buf).unwrap_or_default();
        let common = if skill["scope"] == "workspace" {
            discover::workspace_escode_root(Path::new(workspace))
        } else {
            discover::user_escode_root(&self.home)
        };
        let leaf = source
            .file_name()
            .map(std::borrow::ToOwned::to_owned)
            .unwrap_or_default();
        let target = common.join(&leaf);
        if discover::exists(&target) {
            return fail(format!(
                "通用目录已存在同名技能: {}",
                leaf.to_string_lossy()
            ));
        }
        let (from, to) = (source.clone(), target.clone());
        tokio::task::spawn_blocking(move || copy_tree(&from, &to))
            .await
            .map_err(|e| SkillsError(e.to_string()))?
            .map_err(|e| SkillsError(e.to_string()))?;
        Ok(json!({"newPath": target.join(SKILL_FILE_NAME).to_string_lossy()}))
    }

    pub async fn remove_from_common(&self, workspace: &str, skill_id: &str) -> Result<()> {
        let skill = self.find(workspace, skill_id, true).await?;
        let lower = |p: &Path| p.to_string_lossy().replace('\\', "/").to_lowercase();
        let skill_path = lower(Path::new(skill["path"].as_str().unwrap_or_default()));
        let user_common = lower(&discover::user_escode_root(&self.home));
        let workspace_common = lower(&discover::workspace_escode_root(Path::new(workspace)));
        if !skill_path.contains(&format!("{user_common}/"))
            && !skill_path.contains(&format!("{workspace_common}/"))
        {
            return fail("该技能不在通用目录中");
        }
        let dir = Path::new(skill["path"].as_str().unwrap_or_default())
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default();
        remove_all(&dir).await
    }

    pub async fn delete_skill(&self, workspace: &str, skill_id: &str) -> Result<()> {
        let skill = self.find(workspace, skill_id, true).await?;
        if skill["scope"] == "plugin" {
            return fail("插件提供的技能不可单独删除，请卸载对应插件");
        }
        let raw = skill["sourcePath"]
            .as_str()
            .or_else(|| skill["path"].as_str())
            .unwrap_or_default();
        let skill_dir = Path::new(raw)
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default();
        let leaf = skill_dir
            .file_name()
            .map(std::borrow::ToOwned::to_owned)
            .unwrap_or_default();
        let parent = skill_dir
            .parent()
            .and_then(|p| std::fs::canonicalize(p).ok());
        let not_deletable = || {
            SkillsError(format!(
                "该技能不可删除: {}",
                skill["path"].as_str().unwrap_or_default()
            ))
        };
        let canonical_parent = parent.ok_or_else(not_deletable)?;
        let mut allowed = discover::ancestor_workspace_roots(Path::new(workspace));
        allowed.push(discover::user_escode_root(&self.home));
        allowed.push(discover::user_agents_root(&self.home));
        let contained = allowed.iter().any(|root| {
            let canonical_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.clone());
            canonical_parent.strip_prefix(&canonical_root).is_ok()
        });
        if !contained {
            return Err(not_deletable());
        }
        // 叶子若是软链只删链接、不动目标。
        remove_all(&canonical_parent.join(leaf)).await
    }
}

#[cfg(test)]
#[path = "service_tests.rs"]
mod tests;
