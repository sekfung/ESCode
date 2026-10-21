//! automation 管理服务（本地 SQLite 子集）。

use std::path::PathBuf;

use serde_json::{Value, json};

use super::cron;
use super::repo::AutomationRepo;
use super::repo_write::now_ms;

const MODES: [&str; 10] = [
    "default",
    "yolo",
    "guarded",
    "plan",
    "edit",
    "acceptEdits",
    "auto",
    "dontAsk",
    "bypassPermissions",
    "build",
];

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
        blocking(move || create(&repo, params)).await
    }

    pub async fn update(
        &self,
        id: String,
        params: Value,
        workspace_key: Option<String>,
    ) -> Result<Option<Value>, String> {
        let repo = self.repo.clone();
        blocking(move || update(&repo, &id, params, workspace_key.as_deref())).await
    }

    pub async fn restart(
        &self,
        id: String,
        workspace_key: Option<String>,
    ) -> Result<Option<Value>, String> {
        let repo = self.repo.clone();
        blocking(move || restart(&repo, &id, workspace_key.as_deref())).await
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

fn validate_mode(mode: Option<&str>) -> Result<(), String> {
    if let Some(mode) = mode
        && !mode.is_empty()
        && !MODES.contains(&mode)
    {
        return Err(format!("非法的 mode：{mode}"));
    }
    Ok(())
}

fn validate_schedule_rule(rule: &Value) -> Result<(), String> {
    let unit = rule["unit"]
        .as_str()
        .ok_or_else(|| "调度规则的 unit 不受支持".to_owned())?;
    if !matches!(
        unit,
        "minute" | "hourly" | "daily" | "weekly" | "monthly" | "yearly"
    ) {
        return Err("调度规则的 unit 不受支持".to_owned());
    }
    let interval = rule["interval"]
        .as_i64()
        .filter(|interval| *interval >= 1)
        .ok_or_else(|| "调度规则的 interval 必须是正整数".to_owned())?;
    if unit == "monthly" && interval > 1_200 {
        return Err("monthly interval 不能超过 1200".to_owned());
    }
    if !(0..=23).contains(&rule["hour"].as_i64().unwrap_or(-1)) {
        return Err("调度规则的 hour 必须是 0-23 的整数".to_owned());
    }
    if !(0..=59).contains(&rule["minute"].as_i64().unwrap_or(-1)) {
        return Err("调度规则的 minute 必须是 0-59 的整数".to_owned());
    }
    if let Some(mode) = rule.get("monthlyMode").and_then(Value::as_str)
        && !matches!(mode, "date" | "weekday")
    {
        return Err("调度规则的 monthlyMode 不受支持".to_owned());
    }
    let list_ok = |key: &str, min: i64, max: i64| -> bool {
        rule.get(key)
            .and_then(Value::as_array)
            .is_some_and(|items| {
                !items.is_empty()
                    && items.iter().all(|item| {
                        item.as_i64()
                            .is_some_and(|value| (min..=max).contains(&value))
                    })
            })
    };
    if rule.get("weekdays").is_some() && !list_ok("weekdays", 0, 6) {
        return Err("调度规则的 weekdays 必须是 0-6 的非空整数数组".to_owned());
    }
    if unit == "weekly" && !list_ok("weekdays", 0, 6) {
        return Err("weekly 规则必须包含有效的 weekdays".to_owned());
    }
    if rule.get("monthDays").is_some() && !list_ok("monthDays", 1, 31) {
        return Err("调度规则的 monthDays 必须是 1-31 的非空整数数组".to_owned());
    }
    if rule.get("months").is_some() && !list_ok("months", 1, 12) {
        return Err("调度规则的 months 必须是 1-12 的非空整数数组".to_owned());
    }
    Ok(())
}

fn validate_cron(cron_expr: &str, at: i64) -> Result<(), String> {
    if cron::compute_next_run_at(cron_expr, at).is_none() {
        return Err(format!("非法的 cron 表达式：{cron_expr}"));
    }
    Ok(())
}

fn create(repo: &AutomationRepo, params: Value) -> Result<Value, String> {
    let now = now_ms();
    let mut object = params
        .as_object()
        .cloned()
        .ok_or_else(|| "automation params must be an object".to_owned())?;
    validate_mode(object.get("mode").and_then(Value::as_str))?;
    let relative = object
        .get("relativeDelayMinutes")
        .and_then(Value::as_i64)
        .filter(|minutes| (1..=525_600).contains(minutes));
    if object.get("relativeDelayMinutes").is_some() && relative.is_none() {
        return Err("delayMinutes 必须是 1-525600 的整数".to_owned());
    }
    if let Some(minutes) = relative {
        if object.get("recurring").and_then(Value::as_bool) == Some(true) {
            return Err("相对延迟任务必须设置 recurring=false".to_owned());
        }
        if object.get("maxRuns").is_some() {
            return Err("相对延迟任务固定只运行一次，不能设置 maxRuns".to_owned());
        }
        if object.get("scheduleRule").is_some() {
            return Err("不能同时提交 delayMinutes 和 scheduleRule".to_owned());
        }
        let (cron_expr, rule) = cron::build_relative_delay_schedule(minutes, now);
        object.insert("cronExpr".to_owned(), json!(cron_expr));
        object.insert("scheduleRule".to_owned(), rule);
        object.insert("recurring".to_owned(), Value::Bool(false));
        object.remove("relativeDelayMinutes");
    }
    let mut cron_expr = object
        .get("cronExpr")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    validate_cron(&cron_expr, now)?;
    if let Some(rule) = object.get("scheduleRule").filter(|value| !value.is_null()) {
        validate_schedule_rule(rule)?;
        let mut rule = rule.clone();
        if let Value::Object(fields) = &mut rule {
            fields.insert("anchorAt".to_owned(), json!(now));
        }
        object.insert("scheduleRule".to_owned(), rule);
    } else if let Some(rule) = cron::infer_minute_interval_schedule_rule(&cron_expr, now) {
        object.insert("scheduleRule".to_owned(), rule);
    }
    if cron_expr.is_empty() {
        cron_expr = "* * * * *".to_owned();
        object.insert("cronExpr".to_owned(), json!(cron_expr));
    }
    let recurring = object
        .get("recurring")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let rule = object
        .get("scheduleRule")
        .filter(|value| !value.is_null())
        .cloned();
    let next = cron::compute_initial_next_run_at(&cron_expr, recurring, rule.as_ref(), now)?;
    let end_at = object.get("endAt").and_then(Value::as_i64);
    let ended = end_at.is_some_and(|end| next.is_none_or(|next| next > end));
    let value = Value::Object(object);
    repo.create(
        &value,
        if ended { None } else { next },
        if ended { "completed" } else { "active" },
    )
}

fn update(
    repo: &AutomationRepo,
    id: &str,
    params: Value,
    workspace_key: Option<&str>,
) -> Result<Option<Value>, String> {
    let Some(existing) = repo.get(id, workspace_key)? else {
        return Ok(None);
    };
    let now = now_ms();
    validate_mode(params.get("mode").and_then(Value::as_str))?;
    let existing_cron = existing["cronExpr"].as_str().unwrap_or_default().to_owned();
    let existing_rule = existing
        .get("scheduleRule")
        .filter(|value| !value.is_null())
        .cloned();
    let cron_changed = params
        .get("cronExpr")
        .and_then(Value::as_str)
        .is_some_and(|cron| cron != existing_cron);
    let effective_cron = params
        .get("cronExpr")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| existing_cron.clone());
    validate_cron(&effective_cron, now)?;
    let has_schedule_rule = params.get("scheduleRule").is_some();
    let mut effective_rule = if has_schedule_rule {
        params
            .get("scheduleRule")
            .filter(|value| !value.is_null())
            .cloned()
    } else {
        existing_rule
    };
    if let Some(rule) = &effective_rule {
        validate_schedule_rule(rule)?;
    }
    if cron_changed && !has_schedule_rule {
        effective_rule = cron::infer_minute_interval_schedule_rule(&effective_cron, now);
    }
    let mut recompute = cron_changed || params.get("endAt").is_some() || has_schedule_rule;
    let mut next = if recompute {
        Some(cron::compute_automation_next_run_at(
            &effective_cron,
            effective_rule.as_ref(),
            now,
        ))
    } else {
        None
    };
    let recurring = params
        .get("recurring")
        .and_then(Value::as_bool)
        .unwrap_or_else(|| existing["recurring"].as_bool().unwrap_or(true));
    let recurring_changed = params.get("recurring").is_some() || params.get("maxRuns").is_some();
    let mut lifecycle = None;
    if recurring_changed && next.is_none() {
        // 终态任务改回可跑（或改 maxRuns）时重新排下一轮。
        next = Some(cron::compute_automation_next_run_at(
            &effective_cron,
            effective_rule.as_ref(),
            now,
        ));
        recompute = true;
    }
    let end_at = params
        .get("endAt")
        .and_then(Value::as_i64)
        .or_else(|| existing["endAt"].as_i64());
    if (recompute || recurring_changed)
        && end_at.is_some_and(|end| next.flatten().is_none_or(|next| next > end))
    {
        next = Some(None);
        lifecycle = Some("completed");
    } else if (recompute || recurring_changed) && !recurring && next.flatten().is_none() {
        lifecycle = Some("completed");
    } else if (recompute || recurring_changed)
        && existing["lifecycleStatus"]
            .as_str()
            .is_some_and(|status| status == "completed" || status == "failed")
        && (recurring || next.flatten().is_some())
    {
        lifecycle = Some("active");
    }
    let mut merged = params
        .as_object()
        .cloned()
        .ok_or_else(|| "automation params must be an object".to_owned())?;
    if recompute {
        merged.insert("cronExpr".to_owned(), json!(effective_cron));
        merged.insert(
            "scheduleRule".to_owned(),
            effective_rule.unwrap_or(Value::Null),
        );
    }
    repo.update(
        id,
        &Value::Object(merged),
        next,
        lifecycle,
        recompute,
        workspace_key,
    )
}

fn restart(
    repo: &AutomationRepo,
    id: &str,
    workspace_key: Option<&str>,
) -> Result<Option<Value>, String> {
    let Some(existing) = repo.get(id, workspace_key)? else {
        return Ok(None);
    };
    if existing["lifecycleStatus"].as_str() != Some("failed") {
        return Ok(Some(existing));
    }
    let now = now_ms();
    let rule = existing
        .get("scheduleRule")
        .filter(|value| !value.is_null());
    let next = cron::compute_automation_next_run_at(
        existing["cronExpr"].as_str().unwrap_or_default(),
        rule,
        now,
    );
    repo.restart(id, next, workspace_key)?;
    repo.get(id, workspace_key)
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
