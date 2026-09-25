//! 工具权限能力：静态表来自 TS 工具元数据（scripts/generate-zcode-cli-rust-tool-schemas.mjs 生成），
//! Bash 按只读分类（含 git 运行时上下文）动态降级。
use serde_json::Value;
use std::path::Path;

pub(crate) fn capability(
    cwd: &Path,
    name: &str,
    input: &Value,
) -> Option<zcode_cli_domain::permission::Capability> {
    use zcode_cli_domain::permission::{Capability, Risk};
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
        "low" => zcode_cli_domain::permission::Risk::Low,
        "medium" => zcode_cli_domain::permission::Risk::Medium,
        "high" => zcode_cli_domain::permission::Risk::High,
        _ => zcode_cli_domain::permission::Risk::Critical,
    };
    Some(zcode_cli_domain::permission::Capability {
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
    })
}
