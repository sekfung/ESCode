//! 旧 `plugins` 通道的退役语义（`rust-u2-plugin-management.md` §8）。
//! 对照 TS `createPluginsService`：`getOverview` 回常量，其余 6 个抛退役错，且**一律忽略参数**。
use super::*;

/// 七个成员一个不多一个不少：漏登记会被下面的遍历断言抓到，多登记会被契约对账抓到。
const WRITES: [&str; 6] = [
    "addMarketplace",
    "removeMarketplace",
    "updateMarketplace",
    "installPlugin",
    "uninstallPlugin",
    "setPluginEnabled",
];

#[test]
fn the_channel_exposes_exactly_the_seven_legacy_members() {
    let channel = plugins_channel();
    let mut names = channel.methods();
    names.sort_unstable();
    let mut expected = vec!["getOverview"];
    expected.extend(WRITES);
    expected.sort_unstable();
    assert_eq!(names, expected);
}

/// `createRetiredOverview` 的四个字段逐字对齐（`capability.supported` 必须是 `false`：
/// 回 `true` 会让 UI 以为插件管理可用）。
#[tokio::test]
async fn get_overview_returns_the_retired_constant() {
    let handler = plugins_channel()
        .handler("getOverview")
        .expect("方法表含该成员");
    // 参数故意给空对象：TS 忽略参数，这里也必须照常回常量。
    let Wire::Object(overview) = handler(Wire::Array(vec![Wire::Object(serde_json::json!({}))]))
        .await
        .expect("getOverview 不报错")
    else {
        panic!("overview 应为对象")
    };
    assert_eq!(overview["marketplaces"], serde_json::json!([]));
    assert_eq!(overview["availablePlugins"], serde_json::json!([]));
    assert_eq!(overview["installedPlugins"], serde_json::json!([]));
    assert_eq!(
        overview["capability"],
        serde_json::json!({"supported": false})
    );
    assert_eq!(
        overview.as_object().expect("对象").len(),
        4,
        "只有这四个字段：{overview}"
    );
}

/// 六个写方法都抛退役错，且**缺 workspacePath 也抛退役错而不是 `InvalidRequest`**
/// （TS 形参 `_params` 完全不看；这条与「新通道缺参回 InvalidRequest」的惯例相反）。
///
/// 文案对照**字面量**而非实现里的常量：负向验证发现引用同一个常量会让「改文案」不被抓到，
/// 而这条文案是 TS 侧的可见行为（`throwRetiredPluginManagement`），必须逐字钉住。
#[tokio::test]
async fn retired_writes_throw_the_retirement_error_regardless_of_params() {
    const TS_MESSAGE: &str = "Legacy plugin management has been retired in ESCode Agent mode";
    let channel = plugins_channel();
    for name in WRITES {
        let handler = channel.handler(name).expect("方法表含该成员");
        for arg in [
            // 完全没有参数
            Wire::Array(Vec::new()),
            // 空对象参数
            Wire::Array(vec![Wire::Object(serde_json::json!({}))]),
            // 参数齐全
            Wire::Array(vec![Wire::Object(serde_json::json!({
                "workspacePath": "/w",
                "workspaceIdentity": "/w",
                "pluginName": "p",
                "marketplace": "m",
                "enabled": true,
            }))]),
        ] {
            let error = handler(arg).await.expect_err("退役方法必须报错");
            assert_eq!(error.name, "Error", "{name}");
            assert_eq!(error.message, TS_MESSAGE, "{name}");
        }
    }
}
