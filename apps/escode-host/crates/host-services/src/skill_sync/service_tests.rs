//! skill-sync 服务（`rust-u2-sync.md` §1.2–§1.6）。
use super::*;

fn compare() -> Compare {
    std::sync::Arc::new(|left: &str, right: &str| left.cmp(right))
}

fn write(path: &std::path::Path, content: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

fn frontmatter(name: &str) -> String {
    format!("---\nname: {name}\ndescription: d\n---\nbody\n")
}

fn service(home: &std::path::Path) -> SkillSyncService {
    SkillSyncService::new(home.to_path_buf(), compare())
}

async fn candidate_id(service: &SkillSyncService) -> String {
    let result = service.list_local_user_skill_candidates().await.unwrap();
    result["candidates"][0]["id"].as_str().unwrap().to_owned()
}

#[tokio::test]
async fn export_then_import_syncs_and_the_second_import_skips() {
    let source_home = tempfile::tempdir().unwrap();
    write(
        &source_home
            .path()
            .join(".escode")
            .join("skills")
            .join("alpha")
            .join("SKILL.md"),
        &frontmatter("alpha"),
    );
    write(
        &source_home
            .path()
            .join(".escode")
            .join("skills")
            .join("alpha")
            .join("assets")
            .join("a.bin"),
        "x",
    );
    let target_home = tempfile::tempdir().unwrap();

    let source = service(source_home.path());
    let id = candidate_id(&source).await;
    let exported = source
        .export_skills_archive(&serde_json::json!({"skillIds": [id.clone()]}))
        .await
        .unwrap();
    assert_eq!(
        exported.skills[0]["directoryName"],
        serde_json::json!("alpha")
    );

    let target = service(target_home.path());
    let imported = target
        .import_skills_archive(exported.archive.clone(), false)
        .await
        .unwrap();
    assert_eq!(
        imported["results"][0]["status"],
        serde_json::json!("synced")
    );
    assert_eq!(
        std::fs::read_to_string(
            target_home
                .path()
                .join(".escode")
                .join("skills")
                .join("alpha")
                .join("assets")
                .join("a.bin")
        )
        .unwrap(),
        "x"
    );

    let again = target
        .import_skills_archive(exported.archive, false)
        .await
        .unwrap();
    assert_eq!(again["results"][0]["status"], serde_json::json!("skipped"));
    assert_eq!(
        again["results"][0]["path"],
        serde_json::json!(
            target_home
                .path()
                .join(".escode")
                .join("skills")
                .join("alpha")
                .to_string_lossy()
                .to_string()
        )
    );
    let listed = target.list_local_user_skill_candidates().await.unwrap();
    assert_eq!(listed["candidates"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn import_skips_an_existing_skill_name_in_the_agents_root() {
    let source_home = tempfile::tempdir().unwrap();
    write(
        &source_home
            .path()
            .join(".escode")
            .join("skills")
            .join("alpha")
            .join("SKILL.md"),
        &frontmatter("alpha"),
    );
    let source = service(source_home.path());
    let id = candidate_id(&source).await;
    let exported = source
        .export_skills_archive(&serde_json::json!({"skillIds": [id]}))
        .await
        .unwrap();

    let target_home = tempfile::tempdir().unwrap();
    let existing = target_home
        .path()
        .join(".agents")
        .join("skills")
        .join("other");
    write(&existing.join("SKILL.md"), &frontmatter("alpha"));
    let target = service(target_home.path());
    let imported = target
        .import_skills_archive(exported.archive, false)
        .await
        .unwrap();
    assert_eq!(
        imported["results"][0]["status"],
        serde_json::json!("skipped")
    );
    assert_eq!(
        imported["results"][0]["path"],
        serde_json::json!(existing.to_string_lossy().to_string())
    );
    assert!(
        !target_home
            .path()
            .join(".escode")
            .join("skills")
            .join("alpha")
            .exists(),
        "跳过时不能落下副本"
    );
}

#[tokio::test]
async fn overwrite_is_rejected_and_size_limits_keep_their_phase() {
    let home = tempfile::tempdir().unwrap();
    write(
        &home
            .path()
            .join(".escode")
            .join("skills")
            .join("alpha")
            .join("SKILL.md"),
        "x",
    );
    let service = service(home.path());
    let error = service
        .import_skills_archive(vec![1, 2, 3], true)
        .await
        .unwrap_err();
    assert_eq!(error.message, "skill sync overwrite is not supported");
    assert!(error.size_limit.is_none());

    // archive 阶段：导入体本身超过上限。
    let error = service
        .clone()
        .with_max_archive_bytes(2)
        .import_skills_archive(vec![1, 2, 3], false)
        .await
        .unwrap_err();
    assert_eq!(
        error.message,
        "skill sync size limit exceeded: 3/2 (archive)"
    );

    // selected-content 阶段：候选体积超过上限。
    let small = service.clone().with_max_archive_bytes(0);
    let id = candidate_id(&small).await;
    let error = small
        .export_skills_archive(&serde_json::json!({"skillIds": [id]}))
        .await
        .unwrap_err();
    assert_eq!(error.size_limit.unwrap().2, "selected-content");

    // archive 阶段（导出体 > 选中内容体积）。
    let archive_limited = service.clone().with_max_archive_bytes(1);
    let id = candidate_id(&archive_limited).await;
    let error = archive_limited
        .export_skills_archive(&serde_json::json!({"skillIds": [id]}))
        .await
        .unwrap_err();
    assert_eq!(error.size_limit.unwrap().2, "archive");

    // 未知 id。
    let error = service
        .export_skills_archive(&serde_json::json!({"skillIds": ["missing"]}))
        .await
        .unwrap_err();
    assert_eq!(error.message, "skill sync candidate not found: missing");
}

#[tokio::test]
async fn remote_statuses_check_the_directory_then_fall_back_to_the_skill_name() {
    let home = tempfile::tempdir().unwrap();
    write(
        &home
            .path()
            .join(".escode")
            .join("skills")
            .join("alpha")
            .join("SKILL.md"),
        &frontmatter("alpha"),
    );
    write(
        &home
            .path()
            .join(".agents")
            .join("skills")
            .join("beta")
            .join("SKILL.md"),
        &frontmatter("beta-skill"),
    );
    let service = service(home.path());
    let statuses = service
        .list_remote_user_skill_statuses(&serde_json::json!({
            "directoryNames": ["alpha", "missing"],
        }))
        .await
        .unwrap();
    assert_eq!(statuses["statuses"][0]["exists"], serde_json::json!(true));
    assert_eq!(statuses["statuses"][1]["exists"], serde_json::json!(false));

    let statuses = service
        .list_remote_user_skill_statuses(&serde_json::json!({
            "directoryNames": ["renamed"],
            "skills": [{"directoryName": "renamed", "name": "BETA-SKILL"}],
        }))
        .await
        .unwrap();
    assert_eq!(statuses["statuses"][0]["exists"], serde_json::json!(true));
    assert_eq!(
        statuses["statuses"][0]["path"],
        serde_json::json!(
            home.path()
                .join(".agents")
                .join("skills")
                .join("beta")
                .to_string_lossy()
                .to_string()
        )
    );

    let error = service
        .list_remote_user_skill_statuses(&serde_json::json!({
            "directoryNames": ["../escape"],
        }))
        .await
        .unwrap_err();
    assert_eq!(error.message, "unsafe skill sync path: ../escape");
}

#[tokio::test]
async fn write_access_probe_reports_failures_without_throwing() {
    let home = tempfile::tempdir().unwrap();
    let sync = service(home.path());
    assert_eq!(
        sync.check_remote_user_skill_write_access().await.unwrap()["ok"],
        serde_json::json!(true)
    );
    assert!(home.path().join(".escode").join("skills").exists());
    let leftovers: Vec<_> = std::fs::read_dir(home.path().join(".escode").join("skills"))
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(".escode-sync-preflight-")
        })
        .collect();
    assert!(leftovers.is_empty(), "探测文件必须清理");

    // `.escode` 是文件 → mkdir 失败，返回 ok:false。
    let blocked = tempfile::tempdir().unwrap();
    std::fs::write(blocked.path().join(".escode"), "not a dir").unwrap();
    let result = service(blocked.path())
        .check_remote_user_skill_write_access()
        .await
        .unwrap();
    assert_eq!(result["ok"], serde_json::json!(false));
    assert!(
        result["error"]
            .as_str()
            .is_some_and(|error| !error.is_empty())
    );
}
