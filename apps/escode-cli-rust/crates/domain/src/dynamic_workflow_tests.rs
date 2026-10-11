//! 灰度门的读法（docs/specs/rust-dynamic-workflow.md 第 1 期）：fail-closed、创建参数优先、
//! 翻转不回收已固化的会话。与 OffPeak 同一条规则（TS `appRuntimePreferences` + runtimeConfig）。
use super::*;

const SESSION: &str = "sess_1";

#[test]
fn defaults_to_disabled_and_fixes_at_first_read() {
    let mut policy = Policy::default();
    assert!(!policy.process_enabled());
    // 没有创建参数、也没有工作区策略：fail-closed。
    assert!(!policy.enabled(SESSION));
    // 首次读取即固化：之后翻转进程级结论也不回收本会话。
    policy.update(&[], true, None);
    assert!(policy.process_enabled());
    assert!(!policy.enabled(SESSION));
}

#[test]
fn creation_parameter_wins_over_the_workspace_policy() {
    let mut policy = Policy::default();
    policy.fix(SESSION, true, None);
    assert!(policy.enabled(SESSION));
    // 显式 true 压过进程级结论；显式 false 不压过（`requested || process`）。
    let mut disabled = Policy::default();
    disabled.fix(SESSION, false, None);
    assert!(!disabled.enabled(SESSION));
}

#[test]
fn update_freezes_live_sessions_at_the_previous_conclusion() {
    let mut policy = Policy::default();
    let live = vec![SESSION.to_owned()];
    // 从未固化过的活跃会话按旧结论（关闭）固化，再翻转。
    policy.update(&live, true, None);
    assert!(!policy.enabled(SESSION));
    // 新会话继承新结论。
    policy.fix("sess_2", false, None);
    assert!(policy.enabled("sess_2"));
    // 已固化的会话再翻转一次也不回收。
    policy.update(&live, false, None);
    assert!(policy.enabled("sess_2"));
}

#[test]
fn tool_names_match_the_ts_gate() {
    // 关闭态下架的十一个工具（v3.15.1 加入 FillWorkflowHole）；旧的 `Workflow` 工具不在列。
    assert_eq!(TOOLS.len(), 11);
    assert!(TOOLS.contains(&"FillWorkflowHole"));
    assert!(!TOOLS.contains(&"Workflow"));
    assert!(TOOLS.contains(&"CreateWorkflow"));
    assert!(TOOLS.contains(&"ListSavedWorkflows"));
    assert!(TOOLS.contains(&"ListModels"));
}

#[test]
fn on_demand_mode_follows_ts_runtime_gate() {
    // TS resolveDynamicWorkflowRuntimeGate：参数 → 进程级 → alwaysOn；关闭时不保留 mode。
    let mut policy = Policy::default();
    policy.update(&[], true, Some(true));
    policy.fix("sess_a", false, None);
    assert!(policy.enabled("sess_a") && policy.on_demand("sess_a"));
    policy.fix("sess_b", true, Some(false));
    assert!(!policy.on_demand("sess_b"));
    policy.update(&["sess_a".into(), "sess_b".into()], false, Some(true));
    policy.fix("sess_c", true, None);
    // 关闭时清掉进程级 mode：之后只按布尔开启的会话回到 alwaysOn；已固化会话不回收。
    assert!(!policy.on_demand("sess_c"));
    assert!(policy.on_demand("sess_a"));
    policy.fix("sess_d", false, Some(true));
    assert!(!policy.enabled("sess_d") && !policy.on_demand("sess_d"));
}
