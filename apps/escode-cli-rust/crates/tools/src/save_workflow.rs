//! `SaveWorkflow`（docs/specs/rust-dynamic-workflow.md 第 2/3 期）：对齐 TS core/tool/handlers/save-workflow.ts。
//!
//! 生命周期与 TS 一致：`prepare` = validateInput（名字 / 唯一来源 / 内联脚本不得自带元数据块）→
//! resolveInput（技能门 → `script_path` 读成正文 → 回填 `path` / `overwrite` / `shadowing`）→
//! prepareApproval（编译干净才问；编不过直接交给 handler 回诊断，不弹窗）。`execute` 用**同一个**
//! 检查器（Node 分析子进程）再编译一次：编不过回诊断且不落盘，干净则写盘并按写的那一刻判定覆盖。

use super::saved_workflows as store;
use super::workflow_analyzer::WorkflowAnalyzer;
use crate::contract::ToolOutput;
use crate::domain::json_order::Json;
use crate::domain::saved_workflow as codec;
use anyhow::{Result, anyhow};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

const SOURCE_ERROR: &str = "Provide exactly one script source: `script` for the body inline, or `script_path` for the file holding it (a draft, usually). Passing both, or neither, is ambiguous.";
const SENTINEL_IN_SCRIPT_ERROR: &str = "The `script` must be the workflow body only — it already starts with a `/* escode-workflow` metadata block. Pass the metadata through the `description` / `whenToUse` / `args` fields instead; the block is written for you.";
const NOT_SAVED_NOTE: &str =
    "NOTE: Nothing was saved — fix the errors above and call the tool again.";
pub(crate) const TOOL: &str = "SaveWorkflow";

/// 预处理结论：`Err(文案)` 是交回模型的工具失败（不走权限）；`Ok((入参, ask))` 是归一化入参与
/// 是否需要确认（编译干净才问）。
pub(crate) type Prepared = std::result::Result<(Value, bool), String>;

fn home() -> PathBuf {
    PathBuf::from(escode_cli_host::credential_cipher::node_homedir())
}

fn scope_of(args: &Value) -> store::Scope {
    args["scope"]
        .as_str()
        .and_then(store::Scope::parse)
        .unwrap_or(store::Scope::Project)
}

/// TS `validateSaveWorkflowInput`。
fn validate(args: &Value) -> std::result::Result<(), String> {
    let name = args["name"].as_str().unwrap_or_default();
    if !store::is_valid_name(name) {
        return Err(format!(
            "'{name}' is not a usable workflow name: names may only contain letters, digits, '.', '-' and '_', and must be 1-{} characters.",
            store::MAX_NAME_CHARS
        ));
    }
    if args.get("script").is_some() == args.get("script_path").is_some() {
        return Err(SOURCE_ERROR.into());
    }
    if args["script"]
        .as_str()
        .is_some_and(|script| script.trim_start().starts_with(codec::SENTINEL))
    {
        return Err(SENTINEL_IN_SCRIPT_ERROR.into());
    }
    Ok(())
}

/// TS `describeWorkflowScriptPath`：在工作目录之下给相对路径，否则绝对路径。
pub(crate) fn describe(absolute: &Path, cwd: &Path) -> String {
    match absolute.strip_prefix(cwd) {
        Ok(relative) if !relative.as_os_str().is_empty() => relative.to_string_lossy().into_owned(),
        _ => absolute.to_string_lossy().into_owned(),
    }
}

/// TS `readWorkflowScriptFile`：读一次；`parse_frontmatter` 时带元数据块的文件只留正文（片段整份
/// 就是代码）。返回（绝对路径, 正文）。
pub(crate) fn read_script_file(
    cwd: &Path,
    input: &str,
    parse_frontmatter: bool,
) -> std::result::Result<(PathBuf, String), String> {
    let path = Path::new(input);
    let absolute = super::lexical_path::normalize(&if path.is_absolute() {
        path.to_owned()
    } else {
        cwd.join(path)
    });
    let described = describe(&absolute, cwd);
    let source = std::fs::read_to_string(&absolute).map_err(|error| {
        format!(
            "The workflow script file {described} could not be read: {error}. Pass `path` for a file that exists, or submit the script inline."
        )
    })?;
    let first = source.split('\n').find(|line| !line.trim().is_empty());
    if !parse_frontmatter || first.map(str::trim) != Some(codec::SENTINEL) {
        return Ok((absolute, source));
    }
    match codec::parse(&source) {
        Ok(parsed) => Ok((absolute, parsed.script)),
        Err(failure) => Err(format!(
            "The workflow script file {described} starts with a `{}` metadata block that could not be read ({}): {}. Fix the block in that file, or remove it and pass the script alone.",
            codec::SENTINEL,
            failure.reason,
            failure.detail
        )),
    }
}

/// 技能门（TS `requireDynamicWorkflowSkill`）：拒绝文案点名要重试的工具。
pub(crate) fn skill_gate_message(tool: &str) -> String {
    let skill = super::bundled_skills::DYNAMIC_WORKFLOW_SKILL;
    format!(
        "{tool} needs the `{skill}` skill loaded in this session before it accepts a script. Call the Skill tool with skill \"{skill}\" first — it carries the facade declarations the script is checked against, the authoring rules and this tool's full contract — then call {tool} again. Nothing was started."
    )
}

pub(crate) async fn prepare(
    cwd: &Path,
    args: &Value,
    skill_loaded: bool,
    analyzer: &WorkflowAnalyzer,
) -> Result<Prepared> {
    if let Err(message) = validate(args) {
        return Ok(Err(message));
    }
    if !skill_loaded {
        return Ok(Err(skill_gate_message(TOOL)));
    }
    let mut input = args.clone();
    if let Some(path) = args["script_path"].as_str() {
        match read_script_file(cwd, path, true) {
            Ok((_, script)) => input["script"] = script.into(),
            Err(message) => return Ok(Err(message)),
        }
    }
    let name = args["name"].as_str().unwrap_or_default();
    let scope = scope_of(args);
    let home = home();
    let root = store::root(cwd, scope, &home);
    input["path"] = store::saved_path(&root, name)
        .to_string_lossy()
        .into_owned()
        .into();
    input["overwrite"] = store::exists(cwd, &home, name, Some(scope)).into();
    if let Some(shadowing) = store::shadowing(cwd, &home, name, scope) {
        input["shadowing"] = shadowing.into();
    }
    let script = input["script"].as_str().unwrap_or_default().to_owned();
    let ask = analyzer.analyze(&script).await?["ok"] == true;
    Ok(Ok((input, ask)))
}

pub(crate) async fn execute(
    cwd: &Path,
    input: &Value,
    analyzer: &WorkflowAnalyzer,
) -> Result<ToolOutput> {
    let name = input["name"].as_str().unwrap_or_default().to_owned();
    let scope = scope_of(input);
    let home = home();
    let path = input["path"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| {
            store::saved_path(&store::root(cwd, scope, &home), &name)
                .to_string_lossy()
                .into_owned()
        });
    let script = input["script"]
        .as_str()
        .ok_or_else(|| anyhow!("SaveWorkflow handler received input without a resolved script"))?
        .to_owned();
    let analysis = analyzer.analyze(&script).await?;
    let diagnostics = analysis["diagnostics"].clone();
    let ok = analysis["ok"] == true;
    if !ok {
        let mut lines = vec!["The workflow script has errors:".to_owned()];
        for diagnostic in diagnostics.as_array().into_iter().flatten() {
            lines.push(format!(
                "L{}:C{} {}",
                diagnostic["line"],
                diagnostic["column"],
                diagnostic["message"].as_str().unwrap_or_default()
            ));
        }
        lines.push(String::new());
        lines.push(NOT_SAVED_NOTE.into());
        let response = lines.join("\n");
        let data = json!({
            "name": name,
            "scope": scope.as_str(),
            "path": path,
            "diagnostics": diagnostics,
            "ok": false,
            "response": response,
        });
        return Ok(ToolOutput::new(response, data));
    }
    let mut meta = Json::object();
    meta.set(
        "description",
        Json::str(input["description"].as_str().unwrap_or_default()),
    );
    for key in ["whenToUse", "args"] {
        if let Some(value) = input.get(key) {
            let value = Json::parse(&value.to_string()).unwrap_or(Json::Null);
            meta.set(key, value);
        }
    }
    // 写失败（只读挂载、权限）向上冒泡成工具失败，绝不吞成一个报告了路径的成功输出。
    let written = store::save(cwd, &home, &name, &meta, &script, Some(scope))?;
    let written_path = written.path.to_string_lossy().into_owned();
    let global = written.scope == store::Scope::Global;
    let first = match (written.overwritten, global) {
        (true, true) => format!("Replaced the saved global workflow '{name}' at {written_path}."),
        (true, false) => format!("Replaced the saved workflow '{name}' at {written_path}."),
        (false, true) => format!("Saved global workflow '{name}' to {written_path}."),
        (false, false) => format!("Saved the workflow '{name}' to {written_path}."),
    };
    let response =
        format!("{first} Run it with CreateWorkflow using `saved: {{ name: \"{name}\" }}`.");
    let data = json!({
        "name": name,
        "scope": written.scope.as_str(),
        "path": written_path,
        "diagnostics": diagnostics,
        "ok": true,
        "overwritten": written.overwritten,
        "response": response,
    });
    Ok(ToolOutput::new(response, data))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validation_messages_match_ts() {
        assert_eq!(
            validate(&json!({"name": "bad name", "script": "x"})).unwrap_err(),
            "'bad name' is not a usable workflow name: names may only contain letters, digits, '.', '-' and '_', and must be 1-64 characters."
        );
        assert_eq!(validate(&json!({"name": "ok"})).unwrap_err(), SOURCE_ERROR);
        assert_eq!(
            validate(&json!({"name": "ok", "script": "x", "script_path": "y"})).unwrap_err(),
            SOURCE_ERROR
        );
        assert_eq!(
            validate(&json!({"name": "ok", "script": "\n  /* escode-workflow\n*/"})).unwrap_err(),
            SENTINEL_IN_SCRIPT_ERROR
        );
        assert!(validate(&json!({"name": "ok", "script": "x"})).is_ok());
    }
}
