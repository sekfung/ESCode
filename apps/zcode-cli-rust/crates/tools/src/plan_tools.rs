//! EnterPlanMode / ExitPlanMode 的模型定义与批准计划的落盘（执行由会话 owner 完成）。
use serde_json::{Value, json};
use std::path::Path;

pub(super) fn definitions(schemas: &Value) -> Vec<Value> {
    let descriptions: Value = serde_json::from_str(include_str!("plan_mode_descriptions.json"))
        .expect("validated plan mode descriptions");
    ["EnterPlanMode", "ExitPlanMode"]
        .into_iter()
        .map(|name| json!({"type":"function","function":{"name":name,"description":descriptions[name],"parameters":schemas[name]}}))
        .collect()
}

/// TS writeApprovedPlanFile：`<workspace>/.zcode/plans/<file_name>`，建父目录后原子替换。
pub(super) async fn write_plan_file(cwd: &Path, file_name: &str, plan: &str) -> anyhow::Result<()> {
    anyhow::ensure!(!plan.trim().is_empty(), "ExitPlanMode plan cannot be empty");
    let dir = cwd.join(".zcode").join("plans");
    tokio::fs::create_dir_all(&dir).await?;
    let temp = dir.join(format!("{file_name}.tmp"));
    tokio::fs::write(&temp, plan).await?;
    tokio::fs::rename(&temp, dir.join(file_name)).await?;
    Ok(())
}
