//! 工具权限能力：静态表来自 TS 工具元数据（scripts/generate-escode-cli-rust-tool-schemas.mjs 生成），
//! Bash 按只读分类（含 git 运行时上下文）动态降级。
use serde_json::Value;
use std::path::Path;

pub(crate) fn capability(
    cwd: &Path,
    name: &str,
    input: &Value,
) -> Option<escode_cli_domain::permission::Capability> {
    use escode_cli_domain::permission::{Capability, Risk};
    // 与 TS resolveBashPermissionCapability 一致：只读命令（含 git 上下文安全判定）
    // 降级为 low/none/无需确认，build 模式可直接执行；否则回落到静态能力表。
    if name == "Bash"
        && let Some(command) = input["command"].as_str()
        && crate::bash_git_safety::is_readonly_in_context(command, Some(cwd))
    {
        return Some(Capability {
            read_only: Some(true),
            destructive: Some(false),
            needs_approval: Some(false),
            risk_level: Some(Risk::Low),
            side_effect_scope: Some("none".into()),
            permission_name: Some("bash".into()),
            ..Default::default()
        });
    }
    let table: Value = serde_json::from_str(include_str!("tool_capabilities.json"))
        .expect("generated tool capabilities");
    let entry = table.get(name)?;
    let risk = |s: &str| match s {
        "low" => escode_cli_domain::permission::Risk::Low,
        "medium" => escode_cli_domain::permission::Risk::Medium,
        "high" => escode_cli_domain::permission::Risk::High,
        _ => escode_cli_domain::permission::Risk::Critical,
    };
    Some(escode_cli_domain::permission::Capability {
        allowed_in_plan_mode: entry["allowedInPlanMode"].as_bool(),
        always_ask: entry["alwaysAsk"].as_bool(),
        read_only: entry["readOnly"].as_bool(),
        destructive: entry["destructive"].as_bool(),
        requires_user_interaction: entry["requiresUserInteraction"].as_bool(),
        side_effect_scope: entry["sideEffectScope"].as_str().map(str::to_owned),
        risk_level: entry["riskLevel"].as_str().map(risk),
        needs_approval: entry["needsApproval"].as_bool(),
        permission_name: entry["permissionName"].as_str().map(str::to_owned),
        permission_capability_group: None,
        user_approval_rule: None,
    })
}

/// TS `resolveBashPermissionCapability` 的 guarded 分支：方言取会话 Shell 选择（与 Bash 执行同一解析；
/// 尚未请求过 Host 偏好时按自动探测），命中即回 `safety.bash.*` 规则 id。
pub(crate) fn guarded_rule(input: &Value, shell_override: Option<&Value>) -> Option<String> {
    use escode_cli_domain::guarded::{Dialect, Outcome, match_dangerous_command};
    let command = input["command"].as_str().filter(|c| !c.is_empty())?;
    let over = shell_override.and_then(crate::shell_select::parse_override);
    let env: Vec<(String, String)> = escode_cli_core_api::env::vars();
    let selection = crate::shell_select::resolve(
        crate::shell_select::Platform::current(),
        &env,
        over.as_ref(),
        &|p| Path::new(p).is_file(),
    );
    let dialect = match selection.dialect {
        crate::shell_select::Dialect::Posix => Dialect::Posix,
        crate::shell_select::Dialect::GitBash => Dialect::GitBash,
        crate::shell_select::Dialect::Cmd => Dialect::Cmd,
        crate::shell_select::Dialect::Legacy => Dialect::LegacyShell,
    };
    match match_dangerous_command(command, dialect) {
        Outcome::Matched { rule_id, .. } => Some(rule_id),
        Outcome::NotMatched | Outcome::Unsupported => None,
    }
}
