//! TaskOutput 的模型面文本：TS `formatTaskOutputModelContent`（core/src/tool/handlers/task-output.ts）。
//! 结构化结果仍作 `data` 交给 App；模型读到的是这几段 XML 块。
use serde_json::Value;

const DEFAULT_LENGTH: usize = 32_000;
const MAX_LENGTH: usize = 160_000;

pub fn model_content(result: &Value) -> String {
    let mut blocks = vec![format!(
        "<retrieval_status>{}</retrieval_status>",
        result["retrieval_status"].as_str().unwrap_or_default()
    )];
    let task = &result["task"];
    if task.is_object() {
        let field = |key: &str| task[key].as_str().unwrap_or_default().to_owned();
        blocks.push(format!("<task_id>{}</task_id>", field("task_id")));
        blocks.push(format!("<task_type>{}</task_type>", field("task_type")));
        blocks.push(format!("<status>{}</status>", field("status")));
        if let Some(code) = task["exitCode"].as_i64() {
            blocks.push(format!("<exit_code>{code}</exit_code>"));
        }
        let output = field("output");
        if !output.trim().is_empty() {
            // 没有真实完整文件时 task_id 不是可读取路径，保留原文。
            let content = match task["outputFile"].as_str().filter(|p| !p.is_empty()) {
                Some(path) => truncate(&output, path, std::env::var("TASK_MAX_OUTPUT_LENGTH").ok()),
                None => output,
            };
            blocks.push(format!("<output>\n{}\n</output>", content.trim_end()));
        }
        if let Some(error) = task["error"].as_str().filter(|e| !e.is_empty()) {
            blocks.push(format!("<error>{error}</error>"));
        }
    }
    blocks.join("\n\n")
}

/// TS truncateTaskOutput：超出预算时保留尾部，并指向完整文件。长度按 UTF-16 码元计（JS 字符串长度）。
fn truncate(output: &str, path: &str, configured: Option<String>) -> String {
    let max = configured
        .and_then(|v| v.trim().parse::<i64>().ok())
        .filter(|n| *n > 0)
        .map_or(DEFAULT_LENGTH, |n| (n as usize).min(MAX_LENGTH));
    let units: Vec<u16> = output.encode_utf16().collect();
    if units.len() <= max {
        return output.to_owned();
    }
    let prefix = format!("[Truncated. Full output: {path}]\n\n");
    let tail = max.saturating_sub(prefix.encode_utf16().count());
    format!(
        "{prefix}{}",
        String::from_utf16_lossy(&units[units.len() - tail..])
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn running_task_has_no_output_block() {
        let text = model_content(&json!({"retrieval_status":"not_ready","task":{
            "task_id":"exec_1","task_type":"local_bash","status":"running","output":"","exitCode":null}}));
        assert_eq!(
            text,
            "<retrieval_status>not_ready</retrieval_status>\n\n<task_id>exec_1</task_id>\n\n<task_type>local_bash</task_type>\n\n<status>running</status>"
        );
    }

    #[test]
    fn long_output_keeps_the_tail_and_points_at_the_file() {
        let output = "x".repeat(50);
        let text = truncate(&output, "/o.log", Some("40".into()));
        assert!(text.starts_with("[Truncated. Full output: /o.log]\n\n"));
        assert_eq!(text.encode_utf16().count(), 40);
    }
}
