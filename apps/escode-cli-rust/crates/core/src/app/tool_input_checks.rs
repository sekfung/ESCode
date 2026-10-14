//! 工具调用的输入准备与校验（从 tool_dispatch.rs 拆出）：prepare / checked_input / 别名解析。

use crate::contract::ToolPort;
use anyhow::Result;
use serde_json::Value;

/// 工具预处理（`ToolPort::prepare_tool`）：拒绝时返回交回模型的失败输出；放行时改写 `call` 的参数，
/// 并以临时键 `_escode_approval_proceed` 交回审批门结论（调用方取走后即删除）。
pub(super) async fn prepare(
    tools: &dyn ToolPort,
    session: &str,
    name: &str,
    call: &mut Value,
    skill_loaded: bool,
) -> Result<Option<crate::contract::ToolOutput>> {
    let Ok(args) =
        serde_json::from_str::<Value>(call["function"]["arguments"].as_str().unwrap_or(""))
    else {
        return Ok(None);
    };
    match tools
        .prepare_tool(session, name, &args, skill_loaded)
        .await?
    {
        None => Ok(None),
        Some(Err(message)) => Ok(Some(crate::contract::ToolOutput {
            failed: true,
            ..crate::contract::ToolOutput::text(format!(
                "<tool_use_error>{message}</tool_use_error>"
            ))
        })),
        Some(Ok((input, ask))) => {
            call["function"]["arguments"] = input.to_string().into();
            call["_escode_approval_proceed"] = (!ask).into();
            Ok(None)
        }
    }
}

/// `Err(文案)`：校验失败；`Ok(Some(参数原文))`：去掉未知键后的参数；`Ok(None)`：原样执行。
/// 定义中没有该工具时不校验。
pub(super) fn checked_input(
    definitions: &[Value],
    name: &str,
    call: &Value,
) -> Result<Option<String>, String> {
    use crate::domain::{json_order::Json, schema_order, tool_input_validation as validation};
    let Some(schema) = definitions
        .iter()
        .find(|d| d["function"]["name"] == name)
        .and_then(|d| schema_order::ordered(&d["function"]["parameters"]))
    else {
        return Ok(None);
    };
    // 参数按原文解析以保留键顺序（多余参数按模型给出的顺序列出）；非法 JSON 交给原有路径报错。
    let Some(args) = call["function"]["arguments"].as_str().and_then(Json::parse) else {
        return Ok(None);
    };
    use crate::domain::tool_input_strip::{fill_defaults, strip};
    // TS：内置工具先经 runtime schema（丢弃未知键、填默认值），通过后再按 JSON Schema 校验；
    // runtime 失败时按原始参数报告（有默认值的缺失属性不报）。MCP 工具按原始参数校验。
    let builtin = !name.starts_with("mcp__");
    let normalize = |value: &Json| {
        let mut value = value.clone();
        if builtin {
            fill_defaults(&mut value, &schema);
        }
        value
    };
    // TS runtime schema 的 preprocess/transform 先于丢弃未知键与 JSON 校验；执行也用转换后的参数。
    let coerced = if builtin {
        crate::domain::tool_input_strip::coerce(name, &args)
    } else {
        args.clone()
    };
    let stripped = strip(name, &coerced, &schema);
    if !validation::validate(&normalize(&stripped), &schema).is_empty() {
        let issues = validation::validate(&normalize(&args), &schema);
        if !issues.is_empty() {
            return Err(validation::model_content(name, &issues));
        }
    }
    Ok((stripped != args).then(|| stripped.compact()))
}

/// TS ToolRegistry.get + call-runner 的 registry miss：别名（TaskStop / TaskOutput 的旧名）映射到规范名，
/// 本会话工具面里没有的名字回 `Tool not found: <name>`；空名回 provider 原样的 `No such tool available`。
pub(super) fn resolve_registered(definitions: &[Value], name: &mut String) -> Result<(), String> {
    const ALIASES: [(&str, &str); 6] = [
        ("KillShell", "TaskStop"),
        ("KillBash", "TaskStop"),
        ("AgentOutputTool", "TaskOutput"),
        ("BashOutputTool", "TaskOutput"),
        ("AgentOutput", "TaskOutput"),
        ("BashOutput", "TaskOutput"),
    ];
    let registered = |n: &str| definitions.iter().any(|d| d["function"]["name"] == n);
    if name.trim().is_empty() {
        return Err(format!(
            "<tool_use_error>Error: No such tool available: {name}</tool_use_error>"
        ));
    }
    if registered(name) {
        return Ok(());
    }
    match ALIASES
        .iter()
        .find(|(alias, target)| *alias == name.as_str() && registered(target))
    {
        Some((_, target)) => {
            *name = (*target).to_owned();
            Ok(())
        }
        None => Err(format!("Tool not found: {name}")),
    }
}
