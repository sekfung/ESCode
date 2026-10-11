//! tool_skills.rs 的 visualize 技能门禁单测（身份判定对照 Node `bootstrap/src/app/visualize-skill-gate.ts`）。
use super::*;

#[test]
fn official_visualize_hidden_only_without_inline_host() {
    let official = crate::plugin_marketplace::OFFICIAL_MARKETPLACE;
    assert!(hides_visualize_skills("visualize", official, false));
    assert!(!hides_visualize_skills("visualize", official, true));
}

#[test]
fn other_plugins_and_custom_same_name_skills_kept() {
    let official = crate::plugin_marketplace::OFFICIAL_MARKETPLACE;
    // 同一市场的其他官方插件不隐藏。
    assert!(!hides_visualize_skills("skill-creator", official, false));
    // 用户/项目自定义市场里的同名技能不隐藏（Node 只过滤官方插件身份）。
    assert!(!hides_visualize_skills("visualize", "my-market", false));
    assert!(!hides_visualize_skills("visualize", "", false));
}
