//! 首启导入提示状态（TS `getFirstRunPromptState` / `markFirstRunPromptHandled`）。
//!
//! 状态存放在设置文件的顶层键 `settingsSyncFirstRunPromptHandled`；
//! 记录的是「提示是否已被消费」，不是「导入是否成功」。

use std::path::Path;

use serde_json::{Value, json};

fn error(error: anyhow::Error) -> String {
    format!("{error:#}")
}

pub(super) async fn state(home: &Path) -> Result<Value, String> {
    let settings = crate::settings::read(home).await.map_err(error)?;
    Ok(json!({
        "handled": settings.get("settingsSyncFirstRunPromptHandled") == Some(&Value::Bool(true)),
    }))
}

pub(super) async fn mark_handled(home: &Path) -> Result<(), String> {
    crate::settings::apply_patch(home, &json!({"settingsSyncFirstRunPromptHandled": true}))
        .await
        .map(|_| ())
        .map_err(error)
}

#[cfg(test)]
#[path = "first_run_tests.rs"]
mod tests;
