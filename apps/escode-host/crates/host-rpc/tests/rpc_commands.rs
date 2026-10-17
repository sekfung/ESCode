//! `commands` 端到端（真 WebSocket，`rust-u2-commands.md` §4）。
//!
//! 本片只有 1/6 个成员，所以这里同时钉住「缺口显式可见」：未迁移的 5 个方法必须回显式
//! `NotImplemented` 并列出已实现集合（渲染器据此诊断，而不是挂起）。
mod support;
use support::*;

#[tokio::test(flavor = "multi_thread")]
async fn primary_directory_is_created_and_the_rest_report_explicit_gaps() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let data_base_dir = dir.path().join("data");
    for path in [&home, &data_base_dir] {
        std::fs::create_dir_all(path).unwrap();
    }
    let expected = home.join(".escode").join("commands");
    let surface = start_with_data_base_dir(&home, &data_base_dir).await;
    let mut socket = open(&surface).await;

    // 缺省来源。
    call(
        &mut socket,
        1,
        "commands",
        "getPrimaryUserCommandsDirectory",
        &Wire::Array(Vec::new()),
    )
    .await;
    let Wire::Object(answer) = read_response(&mut socket, 1).await else {
        panic!("应答应为对象")
    };
    assert_eq!(
        answer["path"],
        serde_json::json!(expected.to_string_lossy()),
        "路径 = home + .escode/commands：{answer}"
    );
    assert!(expected.is_dir(), "目录必须真的落盘");

    // 显式 `agentSource`。
    call(
        &mut socket,
        2,
        "commands",
        "getPrimaryUserCommandsDirectory",
        &Wire::Array(vec![Wire::Object(
            serde_json::json!({"agentSource": "escodeAgent"}),
        )]),
    )
    .await;
    let Wire::Object(answer) = read_response(&mut socket, 2).await else {
        panic!("应答应为对象")
    };
    assert_eq!(
        answer["path"],
        serde_json::json!(expected.to_string_lossy())
    );

    // 未知来源：报错（"失败不碰文件系统"由 host-services 的单测覆盖）。
    call(
        &mut socket,
        3,
        "commands",
        "getPrimaryUserCommandsDirectory",
        &Wire::Array(vec![Wire::Object(
            serde_json::json!({"agentSource": "claudeCode"}),
        )]),
    )
    .await;
    let error = read_response(&mut socket, 3)
        .await
        .into_object()
        .expect("错误载荷");
    assert!(
        error["message"]
            .as_str()
            .unwrap_or_default()
            .contains("Unknown command agent source"),
        "{error}"
    );

    // 未迁移的 5 个成员：显式缺口 + 已实现集合。
    for (id, method) in [
        (4, "list"),
        (5, "writeCommandFile"),
        (6, "updateCommandFile"),
        (7, "deleteCommandFile"),
        (8, "setCommandEnabled"),
    ] {
        call(
            &mut socket,
            id,
            "commands",
            method,
            &Wire::Array(vec![Wire::Object(serde_json::json!({}))]),
        )
        .await;
        let error = read_response(&mut socket, id)
            .await
            .into_object()
            .expect("缺口载荷");
        let message = error["message"].as_str().unwrap_or_default();
        assert!(
            message.contains("does not implement `commands."),
            "{method} 应是显式缺口: {error}"
        );
        assert!(
            message.contains("getPrimaryUserCommandsDirectory"),
            "{method} 的缺口应列出已实现集合: {error}"
        );
    }

    surface.shutdown().await;
}
