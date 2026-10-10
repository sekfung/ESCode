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
    policy.update(&[], true);
    assert!(policy.process_enabled());
    assert!(!policy.enabled(SESSION));
}

#[test]
fn creation_parameter_wins_over_the_workspace_policy() {
    let mut policy = Policy::default();
    policy.fix(SESSION, true);
    assert!(policy.enabled(SESSION));
    // 显式 true 压过进程级结论；显式 false 不压过（`requested || process`）。
    let mut disabled = Policy::default();
    disabled.fix(SESSION, false);
    assert!(!disabled.enabled(SESSION));
}

#[test]
fn update_freezes_live_sessions_at_the_previous_conclusion() {
    let mut policy = Policy::default();
    let live = vec![SESSION.to_owned()];
    // 从未固化过的活跃会话按旧结论（关闭）固化，再翻转。
    policy.update(&live, true);
    assert!(!policy.enabled(SESSION));
    // 新会话继承新结论。
    policy.fix("sess_2", false);
    assert!(policy.enabled("sess_2"));
    // 已固化的会话再翻转一次也不回收。
    policy.update(&live, false);
    assert!(policy.enabled("sess_2"));
}

#[test]
fn tool_names_match_the_ts_gate() {
    // 关闭态下架的十个工具；旧的 `Workflow` 工具不在列。
    assert_eq!(TOOLS.len(), 10);
    assert!(!TOOLS.contains(&"Workflow"));
    assert!(TOOLS.contains(&"CreateWorkflow"));
    assert!(TOOLS.contains(&"ListSavedWorkflows"));
    assert!(TOOLS.contains(&"ListModels"));
}
