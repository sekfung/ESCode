//! svn 错误码归类（TS `repo/svnErrors.ts`）。只依赖 `E\d{6}`，不解析本地化文本。

use serde_json::{Value, json};

use super::config::extract_error_codes;

fn issue_for(code: &str) -> Option<&'static str> {
    Some(match code {
        // 工作副本被锁 / 上次操作未完成，需要 cleanup。
        "E155004" | "E155037" | "E200033" => "working-copy-locked",
        // 工作副本过期，需要先 update。
        "E155011" | "E160024" | "E160028" | "E170004" | "E160042" => "out-of-date",
        // 认证。
        "E170001" | "E215004" | "E170013" => "auth-required",
        // 存在未解决冲突。
        "E155015" => "unresolved-conflicts",
        // 不是工作副本。
        "E155007" | "E155010" => "not-working-copy",
        _ => return None,
    })
}

/// E170013 也用于无法连接服务器，只有同时带认证相关错误码时才按认证处理。
pub fn classify(stderr: &str) -> &'static str {
    let codes = extract_error_codes(stderr);
    for code in &codes {
        let Some(issue) = issue_for(code) else {
            continue;
        };
        if code == "E170013"
            && !codes
                .iter()
                .any(|other| other != code && issue_for(other) == Some("auth-required"))
        {
            continue;
        }
        return issue;
    }
    "unknown"
}

/// 只保留 svn 的 `svn: Exxxxxx:` 行作为用户可读信息，去掉噪声并限制长度。
pub fn summarize(stderr: &str) -> String {
    let lines: Vec<&str> = stderr
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    let error_lines: Vec<&str> = lines
        .iter()
        .copied()
        .filter(|line| !extract_error_codes(line).is_empty())
        .collect();
    let chosen = if error_lines.is_empty() {
        lines
    } else {
        error_lines
    };
    let summary = chosen.join("\n");
    if summary.chars().count() > 2_000 {
        let prefix: String = summary.chars().take(2_000).collect();
        format!("{prefix}…")
    } else {
        summary
    }
}

pub fn failure(issue: &str, message: impl Into<String>) -> Value {
    json!({ "ok": false, "issue": issue, "message": message.into() })
}
