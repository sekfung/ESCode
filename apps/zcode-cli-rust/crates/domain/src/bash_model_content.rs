//! Bash 结果的模型可见正文与返回码语义（docs/specs/rust-bash-model-content.md），逐项对齐 TS
//! `core/src/tool/handlers/bash-model-content.ts`、`bash-semantics.ts` 与 `result-persistence-format.ts`。
use serde_json::Value;

const PREVIEW_CHARS: usize = 2_000;
const ASSISTANT_BLOCKING_BUDGET_SECS: u64 = 15;
const SEMANTIC_NON_ERRORS: [&str; 4] = [
    "Condition is false",
    "Files differ",
    "No matches found",
    "Some directories were inaccessible",
];
const NEUTRAL: [&str; 6] = ["", ":", "echo", "false", "printf", "true"];
const SILENT: [&str; 14] = [
    "cd", "chgrp", "chmod", "chown", "cp", "export", "ln", "mkdir", "mv", "rm", "rmdir", "touch", "unset", "wait",
];

/// TS `interpretBashReturnCode`（输出超限在 Rust 中以 stderr 文案表达，不单独解释）。
pub fn return_code_interpretation(command: &str, status: &str, exit_code: Option<i64>) -> Option<String> {
    match status {
        "timed_out" => return Some("Command timed out".into()),
        "cancelled" => return Some("Command was cancelled".into()),
        "spawn_error" => return Some("Command failed to start".into()),
        _ => {}
    }
    match exit_code {
        Some(code) if code != 0 => Some(
            semantic_exit_one(command, code)
                .map(str::to_owned)
                .unwrap_or_else(|| format!("Command exited with code {code}")),
        ),
        _ => None,
    }
}

fn semantic_exit_one(command: &str, code: i64) -> Option<&'static str> {
    if code != 1 {
        return None;
    }
    let analysis = crate::bash_parse::analyze(command);
    if analysis.has_parse_errors || analysis.has_unsupported_syntax {
        return None;
    }
    // 只看决定退出码的最后一条命令：前面出现过 grep 不能把后续 test 的 1 误判为无匹配。
    let last = analysis.commands.last()?;
    let mut name = last.name.as_str();
    if name == "git" {
        let mut args = last.argv.iter().skip(1);
        while let Some(arg) = args.next() {
            if arg.starts_with('-') {
                if arg == "-C" || arg == "-c" {
                    args.next();
                }
                continue;
            }
            if arg == "grep" || arg == "diff" {
                name = if arg == "grep" { "grep" } else { "diff" };
            }
            break;
        }
    }
    match name {
        "egrep" | "fgrep" | "grep" | "rg" => Some("No matches found"),
        "find" => Some("Some directories were inaccessible"),
        "diff" => Some("Files differ"),
        "test" | "[" => Some("Condition is false"),
        _ => None,
    }
}

/// TS `statusFailure` 的执行错误文案：没有 stderr 文本时作为 stderr（TS `stderr.text || error.message`）。
pub fn stop_message(status: &str, timeout_ms: Option<u64>, output_limit: bool) -> Option<String> {
    match status {
        "timed_out" => Some(format!("Command timed out after {}", timeout_duration(timeout_ms.unwrap_or(120_000)))),
        "cancelled" => Some("Execution cancelled".into()),
        _ if output_limit => Some("Execution output exceeded the persisted output limit".into()),
        _ => None,
    }
}

/// TS `formatTimeoutDuration`。
pub fn timeout_duration(ms: u64) -> String {
    let unit = |value: f64| {
        if value.fract() == 0.0 {
            format!("{value}")
        } else {
            let s = format!("{value:.1}");
            s.strip_suffix(".0").map(str::to_owned).unwrap_or(s)
        }
    };
    match ms {
        0..1_000 => format!("{ms}ms"),
        1_000..60_000 => format!("{}s", unit(ms as f64 / 1_000.0)),
        60_000..3_600_000 => format!("{}m", unit(ms as f64 / 60_000.0)),
        _ => format!("{}h", unit(ms as f64 / 3_600_000.0)),
    }
}

/// TS `isSilentBashCommand`。
pub fn is_silent(command: &str) -> bool {
    let analysis = crate::bash_parse::analyze(command);
    if analysis.has_parse_errors || analysis.has_unsupported_syntax || analysis.has_dynamic_words {
        return false;
    }
    let mut counted = false;
    for part in &analysis.commands {
        if part.name.is_empty() {
            continue;
        }
        if part.operator_before == Some("||") && NEUTRAL.contains(&part.name.as_str()) {
            continue;
        }
        counted = true;
        if !SILENT.contains(&part.name.as_str()) {
            return false;
        }
    }
    counted
}

/// TS `isBashProviderErrorStatus`。
pub fn is_provider_error(data: &Value) -> bool {
    data["status"] == "failed"
        && data["exitCode"].as_f64().is_some_and(|code| code != 0.0)
        && !data["returnCodeInterpretation"]
            .as_str()
            .is_some_and(|m| SEMANTIC_NON_ERRORS.contains(&m))
}

/// TS `formatBashModelContent` 的文本分支。
pub fn format(data: &Value) -> String {
    let text = |key: &str| data[key].as_str().unwrap_or_default();
    let mut parts = vec![];
    if is_provider_error(data) {
        parts.push(format!("Exit code {}", data["exitCode"]));
    }
    parts.push(stdout_section(data));
    let mut stderr = text("stderr").trim().to_owned();
    if data["interrupted"] == true {
        if !stderr.is_empty() {
            stderr.push('\n');
        }
        stderr.push_str("<error>Command was aborted before completion</error>");
    }
    parts.push(stderr);
    parts.push(background_section(data));
    parts.retain(|p| !p.is_empty());
    parts.join("\n")
}

fn stdout_section(data: &Value) -> String {
    let stdout = data["stdout"].as_str().unwrap_or_default();
    // TS `replace(/^(\s*\n)+/, "")`：去掉只含空白的前导行。
    let blank = stdout.len() - stdout.trim_start().len();
    let start = stdout[..blank].rfind('\n').map_or(0, |i| i + 1);
    let content = stdout[start..].trim_end();
    let path = (data["status"] != "backgrounded")
        .then(|| data["persistedOutputPath"].as_str().or(data["rawOutputPath"].as_str()))
        .flatten();
    let Some(path) = path else {
        return content.to_owned();
    };
    let bytes = data["persistedOutputSize"].as_u64().unwrap_or_else(|| {
        data["stdoutBytes"].as_u64().unwrap_or(content.len() as u64) + data["stderrBytes"].as_u64().unwrap_or(0)
    });
    persisted_envelope(content, bytes, path)
}

fn background_section(data: &Value) -> String {
    let Some(id) = data["backgroundTaskId"].as_str() else {
        return String::new();
    };
    let path = ["rawOutputPath", "persistedOutputPath", "stdoutPersistedOutputPath", "stderrPersistedOutputPath"]
        .iter()
        .find_map(|k| data[*k].as_str());
    let output = path.map(|p| format!(" Output is being written to: {p}.")).unwrap_or_default();
    if data["assistantAutoBackgrounded"] == true {
        return format!(
            "Command exceeded the assistant-mode blocking budget ({ASSISTANT_BLOCKING_BUDGET_SECS}s) and was moved to the background with ID: {id}. It is still running \u{2014} you will be notified when it completes.{output} In assistant mode, delegate long-running work to a subagent or use run_in_background to keep this conversation responsive."
        );
    }
    if data["backgroundedByUser"] == true {
        return format!(
            "Command was manually backgrounded by user with ID: {id}.{}",
            output.strip_suffix('.').unwrap_or(&output)
        );
    }
    let hint = if output.is_empty() { "" } else { " To check interim output, use Read on that file path." };
    format!("Command running in background with ID: {id}.{output} You will be notified when it completes.{hint}")
}

/// TS `formatPersistedOutputEnvelope`（Bash 字节格式：1024 进制，一位小数去 `.0`）。
pub fn persisted_envelope(content: &str, bytes: u64, path: &str) -> String {
    let (preview, more) = preview(content, PREVIEW_CHARS);
    let mut lines = vec![
        "<persisted-output>".to_owned(),
        format!("Output too large ({}). Full output saved to: {path}", byte_size(bytes)),
        String::new(),
        format!("Preview (first {}):", byte_size(PREVIEW_CHARS as u64)),
        preview.to_owned(),
    ];
    if more {
        lines.push("...".into());
    }
    lines.push("</persisted-output>".into());
    lines.join("\n")
}

/// TS `previewFirstChars`：按 UTF-16 长度计数，后半段有换行时在换行处截断。
fn preview(content: &str, max: usize) -> (&str, bool) {
    if content.encode_utf16().count() <= max {
        return (content, false);
    }
    let mut units = 0;
    let mut end = content.len();
    for (index, c) in content.char_indices() {
        if units + c.len_utf16() > max {
            end = index;
            break;
        }
        units += c.len_utf16();
    }
    let head = &content[..end];
    let cut = head
        .rfind('\n')
        .filter(|&i| head[..i].encode_utf16().count() * 2 > max)
        .unwrap_or(end);
    (&content[..cut], true)
}

fn byte_size(bytes: u64) -> String {
    if bytes == 0 {
        return "0 bytes".into();
    }
    let trim = |v: f64| {
        let s = format!("{v:.1}");
        s.strip_suffix(".0").map(str::to_owned).unwrap_or(s)
    };
    let kb = bytes as f64 / 1024.0;
    if kb < 1.0 {
        return format!("{bytes} bytes");
    }
    if kb < 1024.0 {
        return format!("{}KB", trim(kb));
    }
    let mb = kb / 1024.0;
    if mb < 1024.0 {
        return format!("{}MB", trim(mb));
    }
    format!("{}GB", trim(mb / 1024.0))
}

#[cfg(test)]
#[path = "bash_model_content_tests.rs"]
mod tests;
