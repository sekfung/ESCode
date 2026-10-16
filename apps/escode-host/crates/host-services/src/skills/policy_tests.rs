use super::*;

#[test]
fn policy_matches_the_ts_constants() {
    let golden: serde_json::Value =
        serde_json::from_str(include_str!("policy_golden.json")).expect("golden");
    assert_eq!(golden["skillFileName"], SKILL_FILE_NAME);
    assert_eq!(golden["maxScanDepth"], MAX_SCAN_DEPTH);
    let ids: Vec<&str> = golden["defaultEnabledPluginIds"]
        .as_array()
        .expect("ids")
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert_eq!(
        ids, DEFAULT_ENABLED_PLUGIN_IDS,
        "默认启用的官方插件清单与 TS 漂移"
    );
    for probe in golden["walk"].as_array().expect("walk") {
        let name = probe["name"].as_str().expect("name");
        assert_eq!(should_walk(name), probe["walk"], "{name}");
    }
}
