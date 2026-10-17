//! 写面四法（`rust-u2-commands.md` §1/§3）：真临时 home。
use super::*;
use crate::commands::config::{enabled_overrides, read_config};

fn service() -> (tempfile::TempDir, CommandsWriteService) {
    let dir = tempfile::tempdir().unwrap();
    let service = CommandsWriteService::new(dir.path().to_path_buf());
    (dir, service)
}

fn create(name: &str, prompt: &str) -> Value {
    json!({"config": {"name": name, "prompt": prompt, "description": "说明"}})
}

#[tokio::test]
async fn write_creates_the_file_and_enables_it() {
    let (dir, service) = service();
    let answer = service
        .write_command_file(&create("/report", "写周报"))
        .await
        .unwrap();
    let command = &answer["command"];
    let file_path = dir.path().join(".escode/commands/report.md");
    assert_eq!(command["filePath"], json!(file_path.to_string_lossy()));
    assert_eq!(command["name"], json!("/report"));
    assert_eq!(command["prompt"], json!("写周报"));
    assert_eq!(command["description"], json!("说明"));
    assert_eq!(command["source"], json!("user"));
    assert_eq!(command["scope"], json!("global"));
    assert_eq!(command["enabled"], json!(true));
    assert_eq!(
        command["id"],
        json!("escodeAgent:escode:global:/report"),
        "id = agentSource:location.source:scope:name"
    );
    assert_eq!(command["location"]["scope"], json!("user"));
    assert_eq!(command["location"]["source"], json!("escode"));
    let content = std::fs::read_to_string(&file_path).unwrap();
    assert_eq!(content, "---\ndescription: 说明\n---\n\n写周报");
    // 新建即启用：覆盖表里不该留下这条。
    assert!(enabled_overrides(&read_config(dir.path()).await).is_empty());

    // 同名再建：明确报错（TS 同文案）。
    let error = service
        .write_command_file(&create("/report", "x"))
        .await
        .unwrap_err();
    assert_eq!(error, "Command file already exists: report.md");
}

#[tokio::test]
async fn project_storage_requires_a_workspace_path() {
    let (_dir, service) = service();
    let error = service
        .write_command_file(&json!({
            "config": {"name": "/p", "prompt": "x"},
            "storageLevel": "project",
        }))
        .await
        .unwrap_err();
    assert_eq!(error, "Missing workspace path for project command");
}

/// 改名时把旧的启用覆盖迁到新路径——否则刚禁用的命令会因改名重新启用。
#[tokio::test]
async fn rename_migrates_the_enable_override() {
    let (dir, service) = service();
    service
        .write_command_file(&create("/old-name", "正文"))
        .await
        .unwrap();
    let old_path = dir.path().join(".escode/commands/old-name.md");
    let old_text = old_path.to_string_lossy().into_owned();
    service
        .set_command_enabled(&json!({"filePath": old_text, "enabled": false}))
        .await
        .unwrap();
    assert!(!std::fs::read_to_string(&old_path).unwrap().is_empty());

    let answer = service
        .update_command_file(&json!({
            "config": {"name": "/new-name", "prompt": "正文"},
            "oldFilePath": old_text,
        }))
        .await
        .unwrap();
    let command = &answer["command"];
    assert_eq!(command["name"], json!("/new-name"));
    assert_eq!(command["enabled"], json!(false), "禁用状态跟随改名");
    let new_path = dir.path().join(".escode/commands/new-name.md");
    assert!(!old_path.exists(), "旧文件应被删除");
    assert!(new_path.exists());
    let overrides = enabled_overrides(&read_config(dir.path()).await);
    assert!(
        !overrides
            .get(new_path.to_string_lossy().as_ref())
            .copied()
            .unwrap_or(true)
    );
    assert!(
        !overrides.contains_key(&old_text),
        "旧路径的覆盖要被清掉，不能留悬空键"
    );
}

#[tokio::test]
async fn rename_onto_an_existing_file_is_rejected() {
    let (dir, service) = service();
    service
        .write_command_file(&create("/taken", "x"))
        .await
        .unwrap();
    service
        .write_command_file(&create("/source", "y"))
        .await
        .unwrap();
    let old_path = dir.path().join(".escode/commands/source.md");
    let error = service
        .update_command_file(&json!({
            "config": {"name": "/taken", "prompt": "y"},
            "oldFilePath": old_path.to_string_lossy(),
        }))
        .await
        .unwrap_err();
    assert_eq!(error, "Command file already exists: taken.md");
}

#[tokio::test]
async fn delete_removes_the_file_and_is_idempotent() {
    let (dir, service) = service();
    service
        .write_command_file(&create("/gone", "x"))
        .await
        .unwrap();
    let path = dir.path().join(".escode/commands/gone.md");
    let text = path.to_string_lossy().into_owned();
    service
        .set_command_enabled(&json!({"filePath": text, "enabled": false}))
        .await
        .unwrap();
    service
        .delete_command_file(&json!({"filePath": text}))
        .await
        .unwrap();
    assert!(!path.exists());
    assert!(
        enabled_overrides(&read_config(dir.path()).await).is_empty(),
        "删除也要把覆盖表收敛"
    );
    // 再删一次：文件不存在当作成功。
    service
        .delete_command_file(&json!({"filePath": text}))
        .await
        .unwrap();
}

#[tokio::test]
async fn set_enabled_toggles_only_the_override() {
    let (dir, service) = service();
    let path = dir.path().join(".escode/commands/toggle.md");
    let text = path.to_string_lossy().into_owned();
    service
        .set_command_enabled(&json!({"filePath": text, "enabled": false}))
        .await
        .unwrap();
    let config = read_config(dir.path()).await;
    assert_eq!(config["command"][&text]["enable"], json!(false));
    service
        .set_command_enabled(&json!({"filePath": text, "enabled": true}))
        .await
        .unwrap();
    assert!(
        read_config(dir.path()).await.get("command").is_none(),
        "启用后不应留空 command 对象"
    );
}
