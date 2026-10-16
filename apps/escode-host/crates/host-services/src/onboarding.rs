//! `onboarding-record` 域（docs/specs/rust-u2-user-config-services.md §6；TS `onboardingRecordService.ts`）。
//!
//! 套餐族删除后 TS 的 `loadUserId` 恒为 `null`（apikey/未登录），所以身份只剩匿名一档；
//! `claimAnonymousRecord` 在 `userId=null` 时是 TS 定义的幂等空操作。

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::{Map, Value, json};
use tokio::sync::Mutex;

const OCCUPATIONS: [&str; 14] = [
    "office",
    "developer",
    "independent",
    "infrastructure",
    "product",
    "design",
    "student",
    "creator",
    "operations",
    "marketing",
    "finance",
    "accounting",
    "legal",
    "other",
];

pub struct OnboardingRecords {
    path: PathBuf,
    /// 串行化写：引导保存与并发触发判定同时发生时不丢条目。
    queue: Mutex<()>,
}

type Failure = String;

fn nullable_text(value: Option<&Value>, key: &str, non_empty: bool) -> Result<Value, Failure> {
    match value {
        Some(Value::Null) => Ok(Value::Null),
        Some(Value::String(text)) if !non_empty || !text.is_empty() => {
            Ok(Value::String(text.clone()))
        }
        _ => Err(format!("Invalid onboarding record: {key}")),
    }
}

fn nullable_bool(value: Option<&Value>, key: &str) -> Result<Value, Failure> {
    match value {
        Some(Value::Null) => Ok(Value::Null),
        Some(Value::Bool(flag)) => Ok(Value::Bool(*flag)),
        _ => Err(format!("Invalid onboarding record: {key}")),
    }
}

/// `onboardingRecordEntrySchema`（输出按 schema 键序，未知键被剥离）。
fn parse_entry(value: &Value) -> Result<Value, Failure> {
    let map = value
        .as_object()
        .ok_or("Invalid onboarding record: entry")?;
    let mode = match map.get("interfaceMode") {
        Some(Value::Null) => Value::Null,
        Some(Value::String(mode)) if mode == "coding" || mode == "office" => {
            Value::String(mode.clone())
        }
        _ => return Err("Invalid onboarding record: interfaceMode".into()),
    };
    let completed = match map.get("completedAt") {
        Some(Value::String(text)) if !text.is_empty() => Value::String(text.clone()),
        _ => return Err("Invalid onboarding record: completedAt".into()),
    };
    if map.get("uploadState") != Some(&json!("pending")) {
        return Err("Invalid onboarding record: uploadState".into());
    }
    let mut out = Map::new();
    out.insert(
        "userId".into(),
        nullable_text(map.get("userId"), "userId", true)?,
    );
    out.insert(
        "occupation".into(),
        nullable_text(map.get("occupation"), "occupation", true)?,
    );
    out.insert("interfaceMode".into(), mode);
    out.insert(
        "memoryEnabled".into(),
        nullable_bool(map.get("memoryEnabled"), "memoryEnabled")?,
    );
    out.insert(
        "proactiveSuggestionsEnabled".into(),
        nullable_bool(
            map.get("proactiveSuggestionsEnabled"),
            "proactiveSuggestionsEnabled",
        )?,
    );
    out.insert("completedAt".into(), completed);
    out.insert("uploadState".into(), json!("pending"));
    Ok(Value::Object(out))
}

fn parse_decision(value: &Value) -> Result<Value, Failure> {
    let map = value
        .as_object()
        .ok_or("Invalid onboarding record: decision")?;
    let status = map
        .get("status")
        .and_then(Value::as_str)
        .filter(|s| matches!(*s, "dismissed" | "existing_local_user"));
    let reason = map
        .get("reason")
        .and_then(Value::as_str)
        .filter(|s| matches!(*s, "user_closed" | "existing_local_task"));
    let decided = map
        .get("decidedAt")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty());
    let (Some(status), Some(reason), Some(decided)) = (status, reason, decided) else {
        return Err("Invalid onboarding record: decision".into());
    };
    Ok(json!({
        "userId": nullable_text(map.get("userId"), "userId", true)?,
        "status": status, "reason": reason, "decidedAt": decided,
    }))
}

/// `onboardingRecordFileSchema`：v1 补空 `decisions` 升到 v2。
fn parse_file(value: &Value) -> Result<Value, Failure> {
    let map = value.as_object().ok_or("Invalid onboarding record: file")?;
    let version = map.get("version").and_then(Value::as_u64);
    let device = map
        .get("deviceMid")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or("Invalid onboarding record: deviceMid")?;
    let entries = map
        .get("entries")
        .and_then(Value::as_array)
        .ok_or("Invalid onboarding record: entries")?;
    let entries = entries
        .iter()
        .map(parse_entry)
        .collect::<Result<Vec<_>, _>>()?;
    let decisions = match version {
        Some(1) => Vec::new(),
        Some(2) => map
            .get("decisions")
            .and_then(Value::as_array)
            .ok_or("Invalid onboarding record: decisions")?
            .iter()
            .map(parse_decision)
            .collect::<Result<Vec<_>, _>>()?,
        _ => return Err("Invalid onboarding record: version".into()),
    };
    Ok(json!({"version": 2, "deviceMid": device, "entries": entries, "decisions": decisions}))
}

fn is_anonymous(item: &Value) -> bool {
    item["userId"].is_null()
}

fn has_identity_record(file: &Value) -> bool {
    file["entries"]
        .as_array()
        .is_some_and(|list| list.iter().any(is_anonymous))
        || file["decisions"]
            .as_array()
            .is_some_and(|list| list.iter().any(is_anonymous))
}

fn new_file(device_mid: &str) -> Value {
    json!({"version": 2, "deviceMid": device_mid, "entries": [], "decisions": []})
}

/// `new Date().toISOString()`。
pub fn iso_now() -> String {
    let elapsed = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    iso_from_millis(i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX))
}

pub fn iso_from_millis(millis: i64) -> String {
    let days = millis.div_euclid(86_400_000);
    let rest = millis.rem_euclid(86_400_000);
    // civil_from_days（Howard Hinnant）。
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        rest / 3_600_000,
        rest / 60_000 % 60,
        rest / 1_000 % 60,
        rest % 1_000
    )
}

impl OnboardingRecords {
    /// `<dataBaseDir>/.escode/v2/onboarding-record.json`。
    pub fn path(data_base_dir: &Path) -> PathBuf {
        data_base_dir
            .join(".escode")
            .join("v2")
            .join("onboarding-record.json")
    }

    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            queue: Mutex::new(()),
        }
    }

    /// 不存在或损坏都按“从未记录”处理（TS 同；损坏只告警）。
    async fn read(&self) -> Option<Value> {
        let text = match tokio::fs::read_to_string(&self.path).await {
            Ok(text) => text,
            Err(error) => {
                if error.kind() != std::io::ErrorKind::NotFound {
                    eprintln!("escode-host: read onboarding record failed: {error}");
                }
                return None;
            }
        };
        match serde_json::from_str::<Value>(&text)
            .map_err(|e| e.to_string())
            .and_then(|v| parse_file(&v))
        {
            Ok(file) => Some(file),
            Err(error) => {
                eprintln!(
                    "escode-host: invalid onboarding record json, treating as missing: {error}"
                );
                None
            }
        }
    }

    async fn write(&self, file: &Value) -> Result<(), Failure> {
        let text = serde_json::to_string_pretty(file).map_err(|e| e.to_string())?;
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            escode_host_core::atomic_write::write(&path, text.as_bytes())
        })
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("{e:#}"))
    }

    pub async fn append_record(&self, device_mid: &str, entry: &Value) -> Result<(), Failure> {
        let _queue = self.queue.lock().await;
        let mut file = match self.read().await {
            Some(existing) => {
                if existing["deviceMid"] != device_mid {
                    eprintln!("escode-host: onboarding deviceMid mismatch, keep existing");
                }
                existing
            }
            None => new_file(device_mid),
        };
        let mut record = entry.as_object().cloned().unwrap_or_default();
        record.insert("userId".into(), Value::Null);
        record.insert("uploadState".into(), json!("pending"));
        let validated = parse_entry(&Value::Object(record))?;
        let entries = file["entries"].as_array_mut().ok_or("corrupt entries")?;
        match entries.iter().position(is_anonymous) {
            Some(index) => entries[index] = validated,
            None => entries.push(validated),
        }
        file["decisions"]
            .as_array_mut()
            .ok_or("corrupt decisions")?
            .retain(|d| !is_anonymous(d));
        self.write(&file).await
    }

    pub async fn should_onboard(
        &self,
        device_mid: &str,
        has_existing_local_task: impl std::future::Future<Output = bool>,
    ) -> Result<bool, Failure> {
        if self
            .read()
            .await
            .is_some_and(|file| has_identity_record(&file))
        {
            return Ok(false);
        }
        if !has_existing_local_task.await {
            return Ok(true);
        }
        let _queue = self.queue.lock().await;
        let mut current = self.read().await.unwrap_or_else(|| new_file(device_mid));
        if has_identity_record(&current) {
            return Ok(false);
        }
        current["decisions"].as_array_mut().ok_or("corrupt decisions")?.push(parse_decision(&json!({
            "userId": null, "status": "existing_local_user", "reason": "existing_local_task", "decidedAt": iso_now(),
        }))?);
        self.write(&current).await?;
        Ok(false)
    }

    pub async fn dismiss(&self, device_mid: &str) -> Result<(), Failure> {
        let _queue = self.queue.lock().await;
        let mut file = self.read().await.unwrap_or_else(|| new_file(device_mid));
        if file["entries"]
            .as_array()
            .is_some_and(|list| list.iter().any(is_anonymous))
        {
            return Ok(());
        }
        let decision = parse_decision(&json!({
            "userId": null, "status": "dismissed", "reason": "user_closed", "decidedAt": iso_now(),
        }))?;
        let decisions = file["decisions"]
            .as_array_mut()
            .ok_or("corrupt decisions")?;
        match decisions.iter().position(is_anonymous) {
            Some(index) => decisions[index] = decision,
            None => decisions.push(decision),
        }
        self.write(&file).await
    }

    /// 当前（匿名）用户最近一条作答。
    pub async fn latest_entry(&self) -> Option<Value> {
        self.read().await?["entries"]
            .as_array()?
            .iter()
            .rev()
            .find(|e| is_anonymous(e))
            .cloned()
    }

    /// TS `syncSettingsFromRecord`：回填 settings 的字段范围；无记录 → `None`。
    pub async fn settings_patch(&self) -> Option<Value> {
        let latest = self.latest_entry().await?;
        let occupation = latest["occupation"]
            .as_str()
            .filter(|o| OCCUPATIONS.contains(o))
            .unwrap_or("other");
        Some(json!({
            "onboardingOccupation": occupation,
            "proactiveSuggestionsEnabled": latest["proactiveSuggestionsEnabled"].as_bool().unwrap_or(false),
            "memoryEnabled": latest["memoryEnabled"].as_bool().unwrap_or(false),
        }))
    }

    pub async fn update_preferences(&self, patch: &Value) -> Result<(), Failure> {
        let _queue = self.queue.lock().await;
        let Some(mut file) = self.read().await else {
            return Ok(());
        };
        let entries = file["entries"].as_array_mut().ok_or("corrupt entries")?;
        let Some(index) = entries.iter().rposition(is_anonymous) else {
            return Ok(());
        };
        let mut merged = entries[index].as_object().cloned().unwrap_or_default();
        for (key, value) in patch.as_object().cloned().unwrap_or_default() {
            merged.insert(key, value);
        }
        entries[index] = parse_entry(&Value::Object(merged))?;
        self.write(&file).await
    }

    pub async fn records(&self) -> Option<Value> {
        self.read().await
    }

    pub async fn clear(&self) -> Result<(), Failure> {
        let _queue = self.queue.lock().await;
        match tokio::fs::remove_file(&self.path).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.to_string()),
        }
    }
}

#[cfg(test)]
#[path = "onboarding_tests.rs"]
mod tests;
