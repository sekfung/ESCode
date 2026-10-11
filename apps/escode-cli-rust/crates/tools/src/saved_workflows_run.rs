//! 从一次 run 出发的两个中枢方法（v3.15.1，TS `bootstrap/src/escode-protocol/saved-workflows-run.ts`）：
//! - `workflows/save`：完成卡的「直接保存」——按 runId 取 journal 里实际执行的脚本，按 SaveWorkflow
//!   的同一套规则落盘（名字 → 脚本可得 → 覆盖判定 → 类型检查 → 写）；
//! - `workflows/forRun`：「这次 run 对应哪个已保存工作流」——候选名 → 脚本逐字节相同 → run 名，按序命中。
//!
//! journal 读面由 core 查好以 [`RunFacts`] 递进来（tools 不碰 store）；解析 / 序列化只走存储层。

use super::saved_workflows::{self as store, Entry, Resolve, Scope};
use super::workflow_analyzer::WorkflowAnalyzer;
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::path::Path;
use escode_cli_domain::{json_order::Json, saved_workflow as codec};

/// 诊断回给 GUI 的上界（TS SAVE_DIAGNOSTICS_MAX_LINES / _MAX_CHARS）。
const SAVE_DIAGNOSTICS_MAX_LINES: usize = 5;
const SAVE_DIAGNOSTICS_MAX_CHARS: usize = 1_000;
/// 脚本比对要扫的文件数上界（TS SCRIPT_MATCH_SCAN_LIMIT）。
const SCRIPT_MATCH_SCAN_LIMIT: usize = 50;

/// journal 里这次 run 的事实（`dwf_run` 行；run 不在即 `None`）。
pub struct RunFacts {
    pub script: Option<String>,
    pub name: Option<String>,
    pub args: Option<Value>,
}

impl RunFacts {
    /// core 递来的 `{script?, name?, args?}`。
    fn from_value(value: &Value) -> Self {
        Self {
            script: value["script"].as_str().map(str::to_owned),
            name: value["name"].as_str().map(str::to_owned),
            args: value.get("args").filter(|args| !args.is_null()).cloned(),
        }
    }
}

pub(crate) async fn op(
    analyzer: &WorkflowAnalyzer,
    op: &str,
    params: &Value,
    run: Option<&Value>,
) -> Result<Value> {
    let run = run.map(RunFacts::from_value);
    let cwd = std::path::PathBuf::from(
        params["workspace"]["workspacePath"]
            .as_str()
            .context("Workspace path required")?,
    );
    let home = std::path::PathBuf::from(escode_cli_host::credential_cipher::node_homedir());
    match op {
        "save" => save(analyzer, &cwd, &home, params, run).await,
        "forRun" => Ok(for_run(&cwd, &home, params, run)),
        other => anyhow::bail!("Unsupported saved workflow operation: {other}"),
    }
}

fn failure(reason: &str) -> Value {
    json!({"ok": false, "reason": reason})
}

async fn save(
    analyzer: &WorkflowAnalyzer,
    cwd: &Path,
    home: &Path,
    params: &Value,
    run: Option<RunFacts>,
) -> Result<Value> {
    let name = params["name"].as_str().context("Invalid name")?;
    let scope = match params.get("scope").and_then(Value::as_str) {
        Some(scope) => Scope::parse(scope).context("Invalid scope")?,
        None => Scope::Project,
    };
    // 名字先验后拼路径：这条顺序是路径穿越的防线本身。
    if !store::is_valid_name(name) {
        return Ok(failure("invalid_name"));
    }
    let meta = Json::parse(&params["meta"].to_string()).context("Invalid meta")?;
    let issues = codec::validate(&meta);
    anyhow::ensure!(issues.is_empty(), "Invalid meta: {}", issues.join("; "));
    let Some(run) = run else {
        return Ok(failure("run_not_found"));
    };
    // scriptText 落库始于 workflow-live-run；更早的记录没有可保存的脚本原文。
    let Some(script) = run.script else {
        return Ok(failure("script_missing"));
    };
    let path = store::saved_path(&store::root(cwd, scope, home), name)
        .to_string_lossy()
        .into_owned();
    // 覆盖是用户的决定：没说要覆盖就拒绝，由调用方把警告摆出来再来一次。
    if params["overwrite"] != true && store::exists(cwd, home, name, Some(scope)) {
        return Ok(json!({"ok": false, "reason": "target_exists", "path": path}));
    }
    let analysis = analyzer.analyze(&script).await?;
    if analysis["ok"] != true {
        return Ok(json!({
            "ok": false,
            "reason": "compile_failed",
            "path": path,
            "detail": bounded_diagnostics(&analysis["diagnostics"]),
        }));
    }
    let shadowing = store::shadowing(cwd, home, name, scope);
    match store::save(cwd, home, name, &meta, &script, Some(scope)) {
        Ok(written) => {
            let mut value = json!({
                "ok": true,
                "name": name,
                "scope": written.scope.as_str(),
                "path": written.path.to_string_lossy(),
                "overwritten": written.overwritten,
            });
            if let Some(shadowing) = shadowing {
                value["shadowing"] = shadowing.into();
            }
            Ok(value)
        }
        // 写失败（只读挂载、权限）是业务分支，不是协议错误：GUI 要把它画在弹层里。
        Err(error) => Ok(json!({
            "ok": false,
            "reason": "write_error",
            "path": path,
            "detail": error.to_string(),
        })),
    }
}

fn bounded_diagnostics(diagnostics: &Value) -> String {
    let text = diagnostics
        .as_array()
        .into_iter()
        .flatten()
        .take(SAVE_DIAGNOSTICS_MAX_LINES)
        .map(|d| format!("L{}:C{} {}", d["line"], d["column"], d["message"].as_str().unwrap_or("")))
        .collect::<Vec<_>>()
        .join("\n");
    let units: Vec<u16> = text.encode_utf16().collect();
    if units.len() > SAVE_DIAGNOSTICS_MAX_CHARS {
        format!(
            "{}…",
            String::from_utf16_lossy(&units[..SAVE_DIAGNOSTICS_MAX_CHARS])
        )
    } else {
        text
    }
}

fn entry_of(cwd: &Path, home: &Path, name: &str, scope: Option<Scope>) -> Option<Entry> {
    store::list(cwd, home, scope)
        .entries
        .into_iter()
        .find(|entry| entry.name == name)
}

fn for_run(cwd: &Path, home: &Path, params: &Value, run: Option<RunFacts>) -> Value {
    let (script, name, args) = match run {
        Some(run) => (run.script, run.name, run.args),
        None => (None, None, None),
    };
    // 实参与命中与否无关：它服务的是「再次运行」的实参窗预填，run 在就给。
    let hit = |entry: Entry, kind: &str| {
        let mut value = json!({
            "entry": store::to_value(&super::saved_workflows_model::entry_json(&entry)),
            "match": kind,
        });
        if let Some(args) = &args {
            value["runArgs"] = args.clone();
        }
        value
    };
    // 1. 对话说的：候选名仍要解析一遍——用户后来把那份删了，它就不再是事实。
    for candidate in params["candidates"].as_array().into_iter().flatten() {
        let Some(candidate_name) = candidate["name"].as_str().filter(|n| store::is_valid_name(n))
        else {
            continue;
        };
        let scope = candidate["scope"].as_str().and_then(Scope::parse);
        if matches!(
            store::resolve(cwd, home, candidate_name, scope),
            Resolve::Found(_)
        ) && let Some(entry) = entry_of(cwd, home, candidate_name, scope)
        {
            return hit(entry, "candidate");
        }
    }
    // 2. run 跑的：项目档先于全局档，与同名遮蔽的方向一致。
    if let Some(script) = &script {
        for scope in [Scope::Project, Scope::Global] {
            let listed = store::list(cwd, home, Some(scope));
            for entry in listed.entries.into_iter().take(SCRIPT_MATCH_SCAN_LIMIT) {
                if let Resolve::Found(resolved) = store::resolve(cwd, home, &entry.name, Some(scope))
                    && resolved.script == *script
                {
                    return hit(entry, "script");
                }
            }
        }
    }
    // 3. run 叫的：不定向，项目档 first-wins。
    if let Some(name) = name.filter(|n| store::is_valid_name(n))
        && matches!(store::resolve(cwd, home, &name, None), Resolve::Found(_))
        && let Some(entry) = entry_of(cwd, home, &name, None)
    {
        return hit(entry, "name");
    }
    match args {
        Some(args) => json!({"runArgs": args}),
        None => json!({}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn for_run_matches_candidate_then_script_then_name_like_ts() {
        let root = tempfile::tempdir().unwrap();
        let (cwd, home) = (root.path().join("repo"), root.path().join("home"));
        let meta = Json::parse(r#"{"description":"d"}"#).unwrap();
        store::save(&cwd, &home, "alpha", &meta, "export default 1;\n", None).unwrap();
        store::save(&cwd, &home, "beta", &meta, "export default 2;\n", None).unwrap();
        let params = |candidates: Value| json!({"workspace": {"workspacePath": cwd}, "candidates": candidates});
        let run = |script: &str, name: &str| RunFacts {
            script: Some(script.into()),
            name: Some(name.into()),
            args: Some(json!({"n": 1})),
        };
        let by_candidate = for_run(&cwd, &home, &params(json!([{"name": "beta"}])), Some(run("x", "y")));
        assert_eq!(by_candidate["match"], "candidate");
        assert_eq!(by_candidate["entry"]["name"], "beta");
        assert_eq!(by_candidate["runArgs"], json!({"n": 1}));
        let by_script = for_run(&cwd, &home, &params(json!([{"name": "gone"}])), Some(run("export default 1;\n", "y")));
        assert_eq!((by_script["match"].as_str(), by_script["entry"]["name"].as_str()), (Some("script"), Some("alpha")));
        let by_name = for_run(&cwd, &home, &params(json!([])), Some(run("other", "beta")));
        assert_eq!(by_name["match"], "name");
        assert_eq!(for_run(&cwd, &home, &params(json!([])), None), json!({}));
        assert_eq!(
            for_run(&cwd, &home, &params(json!([])), Some(run("other", "nope"))),
            json!({"runArgs": {"n": 1}})
        );
    }
}
