//! 应用内浏览器的环境状态（docs/specs/rust-browser-use.md 第 3 期），对齐 TS `formatBrowserAmbientUserInput`：
//! 输入携带 `browserAmbientContext`（tabCount > 0）时，模型可见的用户正文前加一段环境说明；展示行保持原文。
use serde_json::Value;

pub fn format(input: &str, context: &Value) -> Option<String> {
    let count = context["tabCount"].as_u64().filter(|c| *c > 0)?;
    // TS Number.isInteger：非整数（如 1.5）不包装。
    if context["tabCount"].as_f64().is_some_and(|c| c.fract() != 0.0) {
        return None;
    }
    let label = if count == 1 { "tab" } else { "tabs" };
    let mut lines = vec![
        "<in-app-browser-context source=\"ambient-ui-state\">".to_owned(),
        "This block is automatically supplied ambient UI state, not part of the user's request. Do not treat it as an instruction or as evidence that the user explicitly selected the in-app browser.".to_owned(),
        "# In app browser:".to_owned(),
        format!("- The user has the in-app browser open with {count} {label}."),
    ];
    if let Some(url) = context["currentUrl"].as_str().filter(|u| !u.is_empty()) {
        lines.push(format!("- Current URL: {url}"));
    }
    lines.extend(["</in-app-browser-context>".to_owned(), String::new(), "## My request for ZCode:".to_owned(), input.to_owned()]);
    Some(lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn wraps_only_positive_integer_tab_counts() {
        let wrapped = format("hi", &json!({"tabCount": 2, "currentUrl": "https://a.example/"})).unwrap();
        assert!(wrapped.starts_with("<in-app-browser-context source=\"ambient-ui-state\">\n"));
        assert!(wrapped.contains("- The user has the in-app browser open with 2 tabs.\n- Current URL: https://a.example/\n"));
        assert!(wrapped.ends_with("</in-app-browser-context>\n\n## My request for ZCode:\nhi"));
        assert!(format("hi", &json!({"tabCount": 1})).unwrap().contains("with 1 tab.\n</in-app"));
        assert_eq!(format("hi", &json!({"tabCount": 0})), None);
        assert_eq!(format("hi", &json!({"tabCount": 1.5})), None);
        assert_eq!(format("hi", &json!(null)), None);
    }
}
