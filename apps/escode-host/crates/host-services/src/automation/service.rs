//! automation 管理服务（本地 SQLite 子集）。

use std::path::PathBuf;

use serde_json::Value;

use super::repo::AutomationRepo;

#[derive(Clone)]
pub struct AutomationService {
    repo: AutomationRepo,
}

impl AutomationService {
    pub fn new(data_base_dir: PathBuf) -> Self {
        Self::from_db_path(super::repo::tasks_index_path(&data_base_dir))
    }

    pub fn from_db_path(path: PathBuf) -> Self {
        Self {
            repo: AutomationRepo::new(path),
        }
    }

    pub async fn list(&self, workspace_key: Option<String>) -> Result<Value, String> {
        let repo = self.repo.clone();
        blocking(move || repo.list(workspace_key.as_deref()).map(Value::Array)).await
    }

    pub async fn delete(&self, id: String, workspace_key: Option<String>) -> Result<Value, String> {
        let repo = self.repo.clone();
        blocking(move || {
            repo.delete(&id, workspace_key.as_deref())?;
            Ok(Value::Null)
        })
        .await
    }

    pub async fn set_enabled(
        &self,
        id: String,
        enabled: bool,
        workspace_key: Option<String>,
    ) -> Result<Value, String> {
        let repo = self.repo.clone();
        blocking(move || {
            repo.set_enabled(&id, enabled, workspace_key.as_deref(), now())?;
            Ok(Value::Null)
        })
        .await
    }

    pub async fn list_runs(
        &self,
        automation_id: String,
        workspace_key: Option<String>,
    ) -> Result<Value, String> {
        let repo = self.repo.clone();
        blocking(move || {
            repo.list_runs(&automation_id, workspace_key.as_deref())
                .map(Value::Array)
        })
        .await
    }

    pub async fn delete_run(
        &self,
        run_id: String,
        workspace_key: Option<String>,
    ) -> Result<Value, String> {
        let repo = self.repo.clone();
        blocking(move || {
            repo.delete_run(&run_id, workspace_key.as_deref())?;
            Ok(Value::Null)
        })
        .await
    }
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

async fn blocking<T, F>(run: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    tokio::task::spawn_blocking(run)
        .await
        .map_err(|error| error.to_string())?
}
