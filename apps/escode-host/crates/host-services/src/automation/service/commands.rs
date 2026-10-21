//! automation 的 create / update / restart 参数规整与计划重算。

use serde_json::{Value, json};

use super::super::cron;
use super::super::repo::AutomationRepo;
use super::super::repo_write::now_ms;
use super::validate::{validate_cron, validate_mode, validate_schedule_rule};

pub(super) fn create(repo: &AutomationRepo, params: Value) -> Result<Value, String> {
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

pub(super) fn update(
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

pub(super) fn restart(
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
