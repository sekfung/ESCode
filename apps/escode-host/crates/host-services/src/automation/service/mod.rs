//! automation 管理服务（本地 SQLite 子集）。
//!
//! 本模块只做 async 入口与阻塞桥接；create/update/restart 的参数规整与校验
//! 在 `commands` / `validate`，避免管理面把校验、日历计算和存储桥接堆在一个文件里。

use std::path::PathBuf;

use serde_json::Value;

use super::repo::AutomationRepo;
use super::repo_write::now_ms;

mod commands;
mod validate;

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

    pub async fn create(&self, params: Value) -> Result<Value, String> {
        let repo = self.repo.clone();
        blocking(move || commands::create(&repo, params)).await
    }

    pub async fn update(
        &self,
        id: String,
        params: Value,
        workspace_key: Option<String>,
    ) -> Result<Option<Value>, String> {
        let repo = self.repo.clone();
        blocking(move || commands::update(&repo, &id, params, workspace_key.as_deref())).await
    }

    pub async fn restart(
        &self,
        id: String,
        workspace_key: Option<String>,
    ) -> Result<Option<Value>, String> {
        let repo = self.repo.clone();
        blocking(move || commands::restart(&repo, &id, workspace_key.as_deref())).await
    }

    pub async fn run_now(
        &self,
        id: String,
        workspace_key: Option<String>,
    ) -> Result<Option<(Value, Value)>, String> {
        let repo = self.repo.clone();
        blocking(move || repo.run_now(&id, workspace_key.as_deref(), now_ms())).await
    }

    pub async fn claim_due(&self, now: i64) -> Result<Value, String> {
        let repo = self.repo.clone();
        blocking(move || repo.claim_due(now).map(Value::Array)).await
    }

    pub async fn mark_dispatched(
        &self,
        id: String,
        dispatched_at: i64,
        next_run_at: Option<i64>,
    ) -> Result<(), String> {
        let repo = self.repo.clone();
        blocking(move || repo.mark_dispatched(&id, dispatched_at, next_run_at)).await
    }

    pub async fn mark_dispatch_failed(
        &self,
        id: String,
        failed_at: i64,
        error: String,
        permanent: bool,
        next_run_at: Option<i64>,
    ) -> Result<(), String> {
        let repo = self.repo.clone();
        blocking(move || repo.mark_dispatch_failed(&id, failed_at, &error, permanent, next_run_at))
            .await
    }

    pub async fn release_claim(&self, id: String) -> Result<(), String> {
        let repo = self.repo.clone();
        blocking(move || repo.release_claim(&id)).await
    }

    pub async fn mark_manual_run_dispatched(
        &self,
        run_id: String,
        session_id: Option<String>,
        dispatched_at: i64,
    ) -> Result<bool, String> {
        let repo = self.repo.clone();
        blocking(move || {
            repo.mark_manual_run_dispatched(&run_id, session_id.as_deref(), dispatched_at)
        })
        .await
    }

    pub async fn fail_manual_run(&self, run_id: String, error: String) -> Result<(), String> {
        let repo = self.repo.clone();
        blocking(move || repo.fail_manual_run(&run_id, &error)).await
    }

    pub async fn mark_run_outcome(
        &self,
        run_id: String,
        outcome: String,
        error: Option<String>,
    ) -> Result<(), String> {
        let repo = self.repo.clone();
        blocking(move || repo.mark_run_outcome(&run_id, &outcome, error.as_deref())).await
    }

    pub async fn upsert_run_claimed(
        &self,
        run_id: String,
        automation_id: String,
        workspace_key: String,
        scheduled_at: i64,
    ) -> Result<(), String> {
        let repo = self.repo.clone();
        blocking(move || {
            repo.upsert_run_claimed(&run_id, &automation_id, &workspace_key, scheduled_at)
        })
        .await
    }

    pub async fn mark_run_dispatch(
        &self,
        run_id: String,
        status: String,
        session_id: Option<String>,
        error: Option<String>,
    ) -> Result<(), String> {
        let repo = self.repo.clone();
        blocking(move || {
            repo.mark_run_dispatch(&run_id, &status, session_id.as_deref(), error.as_deref())
        })
        .await
    }

    pub async fn skip_due(&self, id: String, next_run_at: Option<i64>) -> Result<(), String> {
        let repo = self.repo.clone();
        blocking(move || repo.skip_due(&id, next_run_at)).await
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
