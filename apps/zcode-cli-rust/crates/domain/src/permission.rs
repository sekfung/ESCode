//! 权限判定纯函数：逐位对应 TS `core/src/permission/service.ts::checkPermission`，
//! 见 docs/specs/rust-permission-modes.md。差分 oracle 为 tests/fixtures/permission_matrix.json。
//! 规则匹配见 permission_rules.rs；Bash rulePolicy 与 disallowed/allowedTools 配置尚未接入（均为空）。
pub use crate::permission_rules::{Rule, RuleBehavior, Ruleset};
use crate::permission_rules::{RuleBehavior as B, matches, webfetch_preapproved};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Build,
    Edit,
    Plan,
    Yolo,
    Auto,
}

impl Mode {
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "build" => Self::Build,
            "edit" => Self::Edit,
            "plan" => Self::Plan,
            "yolo" => Self::Yolo,
            "auto" => Self::Auto,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Behavior {
    Allow,
    Ask,
    Deny,
}

impl Behavior {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Ask => "ask",
            Self::Deny => "deny",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Risk {
    Low,
    Medium,
    High,
    Critical,
}

/// 工具声明的能力；`None` 字段按 TS `resolveCapability` 的工具名默认值补齐。
#[derive(Clone, Debug, Default)]
pub struct Capability {
    pub allowed_in_plan_mode: Option<bool>,
    pub always_ask: Option<bool>,
    pub read_only: Option<bool>,
    pub destructive: Option<bool>,
    pub requires_user_interaction: Option<bool>,
    pub side_effect_scope: Option<String>,
    pub risk_level: Option<Risk>,
    pub needs_approval: Option<bool>,
    /// 对应 TS `permission.permission`（如 `edit`、`mcp`）。
    pub permission_name: Option<String>,
    /// 对应 TS `permissionCapabilityGroup`；仅 Host 验证过的官方 CUA 工具携带。
    pub permission_capability_group: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Context<'a> {
    pub tool_name: &'a str,
    pub mode: Mode,
    pub plan_enabled: Option<bool>,
    pub input: &'a Value,
    /// 项目规则（TS `loadProjectPermissionRuleset`）。
    pub project: Option<&'a Ruleset>,
    /// 会话免确认规则，只服务 alwaysAsk 门（TS `sessionRules`）。
    pub session: Option<&'a Ruleset>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Decision {
    pub behavior: Behavior,
    pub rule_id: &'static str,
}

struct Resolved {
    allowed_in_plan_mode: bool,
    always_ask: bool,
    read_only: bool,
    destructive: bool,
    requires_user_interaction: bool,
    side_effect_scope: String,
    risk: Risk,
    needs_approval: bool,
    permission_name: Option<String>,
}

const READ_ONLY_TOOLS: [&str; 7] = [
    "Read",
    "Glob",
    "Grep",
    "WebSearch",
    "WebFetch",
    "TodoRead",
    "TodoWrite",
];
const WRITE_TOOLS: [&str; 4] = ["Write", "Edit", "ApplyPatch", "Bash"];

fn is_read_only(name: &str) -> bool {
    READ_ONLY_TOOLS.contains(&name)
}

fn is_destructive(name: &str) -> bool {
    name == "Bash"
}

fn default_risk(name: &str) -> Risk {
    if is_read_only(name) {
        Risk::Low
    } else if WRITE_TOOLS.contains(&name) {
        Risk::Medium
    } else if is_destructive(name) {
        Risk::High
    } else {
        Risk::Medium
    }
}

fn resolve(name: &str, cap: Option<&Capability>) -> Resolved {
    let default = Capability::default();
    let cap = cap.unwrap_or(&default);
    Resolved {
        allowed_in_plan_mode: cap.allowed_in_plan_mode.unwrap_or(false),
        always_ask: cap.always_ask.unwrap_or(false),
        read_only: cap.read_only.unwrap_or_else(|| is_read_only(name)),
        destructive: cap.destructive.unwrap_or_else(|| is_destructive(name)),
        requires_user_interaction: cap
            .requires_user_interaction
            .unwrap_or_else(|| cap.side_effect_scope.as_deref() == Some("userInteraction")),
        side_effect_scope: cap.side_effect_scope.clone().unwrap_or_else(|| {
            if is_read_only(name) {
                "none".into()
            } else {
                "workspace".into()
            }
        }),
        risk: cap.risk_level.unwrap_or_else(|| default_risk(name)),
        needs_approval: cap.needs_approval.unwrap_or_else(|| !is_read_only(name)),
        permission_name: cap.permission_name.clone(),
    }
}

fn d(behavior: Behavior, rule_id: &'static str) -> Decision {
    Decision { behavior, rule_id }
}

pub fn check(ctx: &Context, cap: Option<&Capability>) -> Decision {
    use Behavior::*;
    let c = resolve(ctx.tool_name, cap);
    let plan_enabled = ctx.plan_enabled.unwrap_or(ctx.mode == Mode::Plan);
    // plan 模式切换工具先于一切能力判断（TS plan-mode-policy.ts）。
    if ctx.tool_name == "EnterPlanMode" {
        return d(Allow, "tool.plan.enter");
    }
    if ctx.tool_name == "ExitPlanMode" && !plan_enabled {
        return d(Deny, "mode.plan.exitOnly");
    }
    let group = cap.and_then(|c| c.permission_capability_group.as_deref());
    let rule =
        |set: Option<&Ruleset>, behavior| matches(set, behavior, ctx.tool_name, ctx.input, group);
    if c.requires_user_interaction {
        return d(Ask, "tool.userInteraction");
    }
    if c.always_ask {
        if ctx.mode == Mode::Auto {
            return d(Deny, "mode.auto.unimplemented");
        }
        if rule(ctx.project, B::Deny) {
            return d(Deny, "rule.project.deny");
        }
        if rule(ctx.session, B::Allow) {
            return d(Allow, "rule.session.allow");
        }
        return d(Ask, "tool.alwaysAsk");
    }
    if ctx.mode == Mode::Yolo && !plan_enabled {
        return d(Allow, "mode.yolo");
    }
    if ctx.mode == Mode::Auto {
        return d(Deny, "mode.auto.unimplemented");
    }
    if rule(ctx.project, B::Deny) {
        return d(Deny, "rule.project.deny");
    }
    if rule(ctx.project, B::Ask) {
        return d(Ask, "rule.project.ask");
    }
    if plan_enabled {
        return check_plan(&c);
    }
    if rule(ctx.project, B::Allow) {
        return d(Allow, "rule.project.allow");
    }
    if webfetch_preapproved(ctx.tool_name, ctx.input) {
        return d(Allow, "tool.webfetch.preapproved");
    }
    if ctx.mode == Mode::Edit
        && c.permission_name.as_deref() == Some("edit")
        && c.side_effect_scope == "workspace"
    {
        return d(Allow, "mode.edit.fileEdit");
    }
    check_build(&c)
}

fn check_plan(c: &Resolved) -> Decision {
    use Behavior::*;
    if c.read_only && !c.destructive {
        return d(Allow, "mode.plan.readOnly");
    }
    if c.permission_name.as_deref() == Some("mcp") && !c.destructive {
        return d(Allow, "mode.plan.mcp");
    }
    if c.allowed_in_plan_mode
        && c.side_effect_scope == "session"
        && !c.destructive
        && !c.needs_approval
    {
        return d(Allow, "mode.plan.explicitSessionCapability");
    }
    d(Deny, "mode.plan.nonReadOnly")
}

fn check_build(c: &Resolved) -> Decision {
    use Behavior::*;
    if c.read_only && !c.destructive && !c.needs_approval {
        return d(Allow, "mode.build.readOnly");
    }
    if c.risk == Risk::Critical {
        return d(Ask, "mode.build.criticalRisk");
    }
    // TS 的 autoApproveHighRisk 默认 false；配置接入后在此读取。
    if c.risk == Risk::High {
        return d(Ask, "mode.build.highRisk");
    }
    if c.side_effect_scope == "session"
        && c.risk == Risk::Low
        && !c.destructive
        && !c.needs_approval
    {
        return d(Allow, "mode.build.sessionState");
    }
    if c.needs_approval || c.destructive || c.side_effect_scope != "none" {
        return d(Ask, "mode.build.sideEffect");
    }
    d(Allow, "mode.build.lowRisk")
}
