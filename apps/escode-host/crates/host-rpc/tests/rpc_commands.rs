//! `commands` 端到端（真 WebSocket，`rust-u2-commands.md` §4）。
//!
//! 覆盖已交付的 5 个成员：目录解析、写/改（改名迁覆盖）/启停/删的命令生命周期，
//! 以及 `list` 仍为**显式缺口**且列出已实现集合（渲染器据此诊断，不挂起）。
mod support;
use support::*;

async fn call_json(socket: &mut Socket, id: i64, method: &str, params: serde_json::Value) -> Wire {
    call(
        socket,
        id,
        "commands",
        method,
        &Wire::Array(vec![Wire::Object(params)]),
    )
    .await;
    read_response(socket, id).await
}

fn path_of(answer: &Wire, key: &str) -> String {
    let Wire::Object(object) = answer else {
        panic!("应答应为对象")
    };
    object[key].as_str().expect("路径字段").to_owned()
}

#[tokio::test(flavor = "multi_thread")]
async fn command_lifecycle_then_list_still_reports_an_explicit_gap() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let data_base_dir = dir.path().join("data");
    for path in [&home, &data_base_dir] {
        std::fs::create_dir_all(path).unwrap();
    }
    let commands_dir = home.join(".escode").join("commands");
    let surface = start_with_data_base_dir(&home, &data_base_dir).await;
    let mut socket = open(&surface).await;

    // ── 首要目录 ────────────────────────────────────────────────────
    let answer = call_json(
        &mut socket,
        1,
        "getPrimaryUserCommandsDirectory",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(path_of(&answer, "path"), commands_dir.to_string_lossy());
    assert!(commands_dir.is_dir(), "目录必须真的落盘");
    // 未知来源报错。
    let error = call_json(
        &mut socket,
        2,
        "getPrimaryUserCommandsDirectory",
        serde_json::json!({"agentSource": "claudeCode"}),
    )
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

    // ── 新建 ────────────────────────────────────────────────────────
    let answer = call_json(
        &mut socket,
        3,
        "writeCommandFile",
        serde_json::json!({"config": {"name": "/weekly", "prompt": "写周报", "description": "周报"}}),
    )
    .await;
    let WeeklyAnswer {
        file_path,
        name,
        enabled,
    } = WeeklyAnswer::from(&answer);
    assert_eq!(name, "/weekly");
    assert!(enabled, "新建即启用");
    assert_eq!(file_path, commands_dir.join("weekly.md").to_string_lossy());
    let content = std::fs::read_to_string(&file_path).unwrap();
    assert_eq!(content, "---\ndescription: 周报\n---\n\n写周报");

    // 同名再建 → 明确报错。
    let error = call_json(
        &mut socket,
        4,
        "writeCommandFile",
        serde_json::json!({"config": {"name": "/weekly", "prompt": "x"}}),
    )
    .await
    .into_object()
    .expect("错误载荷");
    assert_eq!(
        error["message"],
        serde_json::json!("Command file already exists: weekly.md"),
        "{error}"
    );

    // ── 禁用 → 改名（覆盖必须跟随）──────────────────────────────────
    call_json(
        &mut socket,
        5,
        "setCommandEnabled",
        serde_json::json!({"filePath": file_path, "enabled": false}),
    )
    .await;
    let config = std::fs::read_to_string(home.join(".escode/cli/config.json")).unwrap();
    assert!(config.contains("\"enable\": false"), "禁用要落盘: {config}");

    let answer = call_json(
        &mut socket,
        6,
        "updateCommandFile",
        serde_json::json!({
            "config": {"name": "/weekly-report", "prompt": "写周报"},
            "oldFilePath": file_path,
        }),
    )
    .await;
    let renamed = WeeklyAnswer::from(&answer);
    assert_eq!(renamed.name, "/weekly-report");
    assert!(
        !renamed.enabled,
        "禁用状态跟随改名（否则刚禁用就被重新启用）"
    );
    assert!(!std::path::Path::new(&file_path).exists(), "旧文件应被删除");
    let config = std::fs::read_to_string(home.join(".escode/cli/config.json")).unwrap();
    assert!(
        !config.contains("weekly.md"),
        "旧路径的覆盖不能留悬空键: {config}"
    );

    // ── 删除 ────────────────────────────────────────────────────────
    call_json(
        &mut socket,
        7,
        "deleteCommandFile",
        serde_json::json!({"filePath": renamed.file_path}),
    )
    .await;
    assert!(!std::path::Path::new(&renamed.file_path).exists());
    let config = std::fs::read_to_string(home.join(".escode/cli/config.json")).unwrap();
    assert!(
        !config.contains("weekly-report"),
        "删除也要收敛覆盖表: {config}"
    );

    // ── `list` 仍是显式缺口，且列出已实现集合 ────────────────────────
    let error = call_json(&mut socket, 8, "list", serde_json::json!({}))
        .await
        .into_object()
        .expect("缺口载荷");
    let message = error["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("does not implement `commands.list`"),
        "{error}"
    );
    assert!(
        message.contains("writeCommandFile") && message.contains("setCommandEnabled"),
        "缺口要列出已实现集合: {error}"
    );

    surface.shutdown().await;
}

/// 从 `{command: {...}}` 应答里取三项断言用的字段。
struct WeeklyAnswer {
    file_path: String,
    name: String,
    enabled: bool,
}

impl WeeklyAnswer {
    fn from(answer: &Wire) -> Self {
        let Wire::Object(answer) = answer else {
            panic!("应答应为对象")
        };
        let command = &answer["command"];
        Self {
            file_path: command["filePath"].as_str().expect("filePath").to_owned(),
            name: command["name"].as_str().expect("name").to_owned(),
            enabled: command["enabled"].as_bool().expect("enabled"),
        }
    }
}
