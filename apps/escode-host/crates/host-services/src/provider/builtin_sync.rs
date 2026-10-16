//! Built-in Remote 刷新（TS `escode-builtin-remote-synchronizer.ts`，spec §12）：
//! 共享控制文件 `escode-builtin-refresh.json` 合并多进程刷新，**网络期间不持锁**。
//! 控制文件格式与 TS 逐字段相同，TS Host 与 Rust Host 共存时按同一把租约协调。

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use escode_cli_host::file_lock;
use serde::{Deserialize, Serialize};

use super::builtin_download::{JsonGet, TOTAL_BUDGET, download_release};
use super::builtin_source::{ApplyResult, BuiltinSource, ResolvedActive, control_file_path};

const HOUR_MS: u64 = 60 * 60 * 1_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Skip {
    LeaseHeld,
    NotDue,
    EndpointChanged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshResult {
    Updated,
    Unchanged,
    Stale,
    /// Endpoint 没有配置 Release。
    Missing,
    Skipped(Skip),
}

#[derive(Debug, Clone, Copy)]
pub struct Timing {
    pub success_interval_ms: u64,
    pub lease_ms: u64,
    pub failure_base_ms: u64,
    pub failure_max_ms: u64,
    pub budget: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            success_interval_ms: HOUR_MS,
            lease_ms: 30_000,
            failure_base_ms: 60_000,
            failure_max_ms: HOUR_MS,
            budget: TOTAL_BUDGET,
        }
    }
}

/// TS `RefreshControl`（`schemaVersion` 固定 1）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Control {
    schema_version: u8,
    endpoint_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    lease_id: Option<String>,
    lease_until: u64,
    next_eligible_at: u64,
    failure_count: u32,
}

pub struct Synchronizer {
    source: Arc<BuiltinSource>,
    get: Arc<dyn JsonGet>,
    clock: Arc<dyn Fn() -> u64 + Send + Sync>,
    timing: Timing,
    /// 进程内同一时刻只跑一次刷新（TS `#inFlight`）。
    gate: tokio::sync::Mutex<()>,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}

impl Synchronizer {
    pub fn new(source: Arc<BuiltinSource>, get: Arc<dyn JsonGet>) -> Self {
        Self {
            source,
            get,
            clock: Arc::new(now_ms),
            timing: Timing::default(),
            gate: tokio::sync::Mutex::new(()),
        }
    }

    #[cfg(test)]
    pub fn with_clock(mut self, clock: Arc<dyn Fn() -> u64 + Send + Sync>) -> Self {
        self.clock = clock;
        self
    }

    /// TS `refresh({force})`。`Err` = 下载/校验/应用失败（已按退避写入控制文件）。
    pub async fn refresh(&self, force: bool) -> Result<RefreshResult, String> {
        let _in_flight = self.gate.lock().await;
        let active = self
            .source
            .resolve_active()
            .await
            .map_err(|error| format!("{error:#}"))?
            .ok_or_else(|| "ESCode Built-in Active 缓存未启用".to_owned())?;
        let control_path = control_file_path(&active.path);
        let lease_id = uuid::Uuid::new_v4().to_string();
        if let Some(skip) = self
            .acquire(&control_path, &active.endpoint_origin, &lease_id, force)
            .await?
        {
            return Ok(RefreshResult::Skipped(skip));
        }
        let outcome = self.fetch_and_apply(&active).await;
        self.finish(
            &control_path,
            &active.endpoint_origin,
            &lease_id,
            outcome.is_ok(),
        )
        .await?;
        outcome
    }

    async fn fetch_and_apply(&self, active: &ResolvedActive) -> Result<RefreshResult, String> {
        let app_version = self.source.app_version().unwrap_or_default();
        let release = download_release(
            &active.endpoint_origin,
            app_version,
            self.source.platform(),
            self.get.as_ref(),
            self.timing.budget,
        )
        .await?;
        // 下载期间 Endpoint 可能被切换：结果属于旧 Endpoint，丢弃（旧租约按成功收口）。
        let current = self
            .source
            .resolve_active()
            .await
            .map_err(|error| format!("{error:#}"))?;
        if current.as_ref().map(|c| &c.endpoint_origin) != Some(&active.endpoint_origin) {
            return Ok(RefreshResult::Skipped(Skip::EndpointChanged));
        }
        let Some(release) = release else {
            return Ok(RefreshResult::Missing);
        };
        let applied = self
            .source
            .apply_remote_release(release)
            .await
            .map_err(|error| format!("{error:#}"))?;
        Ok(match applied {
            ApplyResult::Updated => RefreshResult::Updated,
            ApplyResult::Unchanged => RefreshResult::Unchanged,
            ApplyResult::Stale => RefreshResult::Stale,
        })
    }

    async fn acquire(
        &self,
        path: &Path,
        endpoint_key: &str,
        lease_id: &str,
        force: bool,
    ) -> Result<Option<Skip>, String> {
        with_lock(path, async {
            let now = (self.clock)();
            let current = read_control(path).await;
            let same = current.endpoint_key == endpoint_key;
            if same && current.lease_until > now {
                return Ok(Some(Skip::LeaseHeld));
            }
            if !force && same && current.next_eligible_at > now {
                return Ok(Some(Skip::NotDue));
            }
            write_control(
                path,
                &Control {
                    schema_version: 1,
                    endpoint_key: endpoint_key.to_owned(),
                    lease_id: Some(lease_id.to_owned()),
                    lease_until: now + self.timing.lease_ms,
                    next_eligible_at: if same { current.next_eligible_at } else { 0 },
                    failure_count: if same { current.failure_count } else { 0 },
                },
            )
            .await?;
            Ok(None)
        })
        .await
    }

    /// 只在租约仍是自己的时才收口（别的进程接管后不能覆盖它的租约）。
    async fn finish(
        &self,
        path: &Path,
        endpoint_key: &str,
        lease_id: &str,
        success: bool,
    ) -> Result<(), String> {
        with_lock(path, async {
            let current = read_control(path).await;
            if current.endpoint_key != endpoint_key || current.lease_id.as_deref() != Some(lease_id)
            {
                return Ok(());
            }
            let failure_count = if success {
                0
            } else {
                current.failure_count + 1
            };
            let delay = if success {
                self.timing.success_interval_ms
            } else {
                let exponent = failure_count.saturating_sub(1).min(32);
                self.timing
                    .failure_base_ms
                    .saturating_mul(1u64 << exponent)
                    .min(self.timing.failure_max_ms)
            };
            write_control(
                path,
                &Control {
                    schema_version: 1,
                    endpoint_key: endpoint_key.to_owned(),
                    lease_id: None,
                    lease_until: 0,
                    next_eligible_at: (self.clock)() + delay,
                    failure_count,
                },
            )
            .await
        })
        .await
    }
}

async fn with_lock<T>(
    path: &Path,
    body: impl std::future::Future<Output = Result<T, String>>,
) -> Result<T, String> {
    if let Some(dir) = path.parent() {
        tokio::fs::create_dir_all(dir)
            .await
            .map_err(|error| error.to_string())?;
    }
    let guard = file_lock::acquire(path)
        .await
        .map_err(|error| format!("{error:#}"))?;
    let outcome = body.await;
    guard.release().await;
    outcome
}

/// 控制文件不是业务事实：缺失/损坏/形状不对都按空控制重建，不能永久阻断刷新。
async fn read_control(path: &Path) -> Control {
    let Ok(text) = tokio::fs::read_to_string(path).await else {
        return Control::default();
    };
    match serde_json::from_str::<Control>(&text) {
        Ok(control) if control.schema_version == 1 => control,
        _ => Control::default(),
    }
}

async fn write_control(path: &Path, control: &Control) -> Result<(), String> {
    let text = serde_json::to_string_pretty(control).map_err(|error| error.to_string())?;
    file_lock::write_private(path, &text)
        .await
        .map_err(|error| format!("{error:#}"))
}

#[cfg(test)]
#[path = "builtin_sync_tests.rs"]
mod tests;
