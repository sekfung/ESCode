//! `usage-stats` 的参数整形与「不拉起运行时」语义（`rust-u2-misc-channels.md` §1）。
use super::*;

/// TS 只发 `{range, timeZone}`：多送的字段必须丢掉（运行时会按未知字段处理），
/// 且 `range` 之外的字段不能漏发（`timeZone` 决定 CLI 的时区偏移计算）。
#[test]
fn only_range_and_time_zone_reach_the_runtime() {
    assert_eq!(
        usage_request_body(&serde_json::json!({
            "range": "7d",
            "timeZone": "Asia/Shanghai",
            "workspacePath": "/w",
            "workspaceIdentity": "/w",
        })),
        serde_json::json!({"range": "7d", "timeZone": "Asia/Shanghai"})
    );
}

/// `timeZone` 是可选入参（`AppUsageRequest.timeZone?`）：缺省与显式 null 都必须**省略键**，
/// 不能下发 null（运行时解析严格）；`range` 同理透传。
#[test]
fn absent_and_null_optional_fields_are_omitted() {
    assert_eq!(
        usage_request_body(&serde_json::json!({"range": "all"})),
        serde_json::json!({"range": "all"})
    );
    assert_eq!(
        usage_request_body(&serde_json::json!({"range": "all", "timeZone": null})),
        serde_json::json!({"range": "all"})
    );
    // 入参整个缺失时不能 panic：回空体，由运行时按自己的缺省判定（TS 同）。
    assert_eq!(usage_request_body(&Value::Null), serde_json::json!({}));
}

/// existing-only：没有活实例时报 `no_active_workspace`，**不**为这次查询拉起运行时。
/// 负向意义：本用例用的是空实例表，若实现改成 start-if-needed，这里会试图启动实例而不是立即报错。
#[tokio::test]
async fn without_a_live_runtime_the_call_reports_no_active_workspace() {
    let channel = usage_stats_channel(escode_host_runtime::Instances::new(
        escode_host_runtime::Link::Embedded,
    ));
    let handler = channel
        .handler("getAppUsageSnapshot")
        .expect("方法表含该成员");
    let error = handler(Wire::Array(vec![Wire::Object(
        serde_json::json!({"range": "30d"}),
    )]))
    .await
    .expect_err("无活实例必须报错");
    assert_eq!(error.name, "Error");
    assert_eq!(error.message, "no_active_workspace");
}
