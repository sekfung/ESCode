//! off-peak 管理服务：把本地 SQLite 状态机包装成 RPC 形状。

use std::path::PathBuf;

use serde_json::Value;

use super::repo::OffPeakRepo;

#[derive(Clone)]
pub struct OffPeakService {
    repo: OffPeakRepo,
}

impl OffPeakService {
    pub fn new(data_base_dir: PathBuf) -> Self {
        Self {
            repo: OffPeakRepo::new(super::repo::tasks_index_path(&data_base_dir)),
        }
    }

    pub async fn list(&self) -> Result<Value, String> {
        let repo = self.repo_path();
        blocking(move || repo.list().map(Value::Array)).await
    }

    pub async fn get(&self, id: String) -> Result<Value, String> {
        let repo = self.repo_path();
        blocking(move || repo.get(&id).map(|task| task.unwrap_or(Value::Null))).await
    }

    pub async fn cancel(&self, id: String) -> Result<Value, String> {
        let repo = self.repo_path();
        blocking(move || {
            repo.cancel(&id, now())
                .map(|task| task.unwrap_or(Value::Null))
        })
        .await
    }

    pub async fn pause(&self, id: String) -> Result<Value, String> {
        let repo = self.repo_path();
        blocking(move || {
            repo.set_paused(&id, true, now())
                .map(|task| task.unwrap_or(Value::Null))
        })
        .await
    }

    pub async fn resume(&self, id: String) -> Result<Value, String> {
        let repo = self.repo_path();
        blocking(move || {
            repo.set_paused(&id, false, now())
                .map(|task| task.unwrap_or(Value::Null))
        })
        .await
    }

    pub async fn delete(&self, id: String) -> Result<Value, String> {
        let repo = self.repo_path();
        blocking(move || {
            if let Some(task) = repo.get(&id)?
                && !matches!(
                    task["status"].as_str(),
                    Some("completed" | "failed" | "cancelled")
                )
            {
                repo.cancel(&id, now())?;
            }
            repo.delete(&id)?;
            Ok(Value::Null)
        })
        .await
    }

    pub async fn delete_history(&self, id: String) -> Result<Value, String> {
        let repo = self.repo_path();
        blocking(move || {
            repo.delete_history(&id, now())
                .map(|task| task.unwrap_or(Value::Null))
        })
        .await
    }

    pub async fn update(&self, id: String, params: Value) -> Result<Value, String> {
        let repo = self.repo_path();
        blocking(move || {
            repo.update(&id, &params, now())
                .map(|task| task.unwrap_or(Value::Null))
        })
        .await
    }

    fn repo_path(&self) -> OffPeakRepo {
        // OffPeakRepo 只持有路径，克隆成本可忽略；每个请求在 spawn_blocking 内独立开连接。
        self.repo.clone()
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
