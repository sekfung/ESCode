//! svn 服务对 RPC 的公开方法：工作副本信息、读写分派与 svn 路径设置。

use serde_json::{Map, Value, json};

use super::runtime::SvnService;
use super::{BinaryState, history, reads, text, writes, writes_merge};

impl SvnService {
    pub async fn get_working_copy_info(&self, params: &Value) -> Value {
        let availability = match self.resolve_binary().await {
            BinaryState::Ready(binary) => json!({
                "status": "available",
                "version": binary.version,
                "binaryPath": binary.binary_path,
            }),
            BinaryState::NotFound => json!({ "status": "not-found" }),
            BinaryState::Unsupported {
                version,
                binary_path,
            } => json!({
                "status": "unsupported-version",
                "version": version,
                "binaryPath": binary_path,
            }),
        };
        let mut result = Map::new();
        result.insert("availability".to_owned(), availability);
        result.insert("isWorkingCopy".to_owned(), Value::Bool(false));
        if !matches!(self.resolve_binary().await, BinaryState::Ready(_)) {
            return Value::Object(result);
        }
        let Ok(workspace) = reads::read_workspace_info(self, params).await else {
            return Value::Object(result);
        };
        result.insert("isWorkingCopy".to_owned(), Value::Bool(true));
        let info = &workspace.info;
        insert_some(&mut result, "wcRoot", info.wc_root.clone());
        insert_some(&mut result, "url", info.url.clone());
        insert_some(&mut result, "repositoryRoot", info.repository_root.clone());
        insert_some(&mut result, "repositoryUuid", info.repository_uuid.clone());
        if let Some(revision) = info.revision {
            result.insert("revision".to_owned(), Value::from(revision));
        }
        if let Some(branch) = &workspace.branch {
            result.insert("branch".to_owned(), branch.to_json());
        }
        result.insert("canSwitch".to_owned(), Value::Bool(workspace.can_switch));
        Value::Object(result)
    }

    pub async fn get_changes(&self, params: &Value) -> Value {
        reads::read_changes(self, params).await
    }

    pub async fn get_diff(&self, params: &Value) -> Value {
        reads::read_diff(self, params).await
    }

    pub async fn get_log(&self, params: &Value) -> Value {
        history::get_log(self, params).await
    }

    pub async fn get_file_log(&self, params: &Value) -> Value {
        history::get_file_log(self, params).await
    }

    pub async fn get_blame(&self, params: &Value) -> Value {
        history::get_blame(self, params).await
    }

    pub async fn get_remote_status(&self, params: &Value) -> Value {
        history::get_remote_status(self, params).await
    }

    pub async fn list_branches(&self, params: &Value) -> Value {
        history::list_branches(self, params).await
    }

    pub async fn commit(&self, params: &Value) -> Value {
        writes::commit(self, params).await
    }

    pub async fn update(&self, params: &Value) -> Value {
        writes::update(self, params).await
    }

    pub async fn revert(&self, params: &Value) -> Value {
        writes::revert(self, params).await
    }

    pub async fn resolve(&self, params: &Value) -> Value {
        writes::resolve(self, params).await
    }

    pub async fn switch_branch(&self, params: &Value) -> Value {
        writes::switch_branch(self, params).await
    }

    pub async fn merge(&self, params: &Value) -> Value {
        writes_merge::merge(self, params).await
    }

    pub async fn cleanup(&self, params: &Value) -> Value {
        writes::cleanup(self, params).await
    }

    pub async fn set_binary_path(&self, params: &Value) -> anyhow::Result<()> {
        let trimmed = text(params, "binaryPath");
        let value = (!trimmed.trim().is_empty()).then(|| Value::String(trimmed.trim().to_owned()));
        crate::settings::set_key(self.home(), "svnBinaryPath", value).await?;
        self.clear_binary_cache().await;
        Ok(())
    }
}

fn insert_some(map: &mut Map<String, Value>, key: &str, value: Option<String>) {
    if let Some(value) = value {
        map.insert(key.to_owned(), Value::String(value));
    }
}
